//! Networking **codec + capture + apply + snapshots** — no lightyear
//! dependency. This is the transport-agnostic wire the lightyear adapter (this
//! same crate's `client`/`server`) drives. It lives behind the `networking`
//! feature; single-player builds (which omit `lunco-networking` entirely) never
//! compile or register it, so they carry no networking resources at all.
//!
//! Flow:
//! - **capture** (`capture_command::<C>`, registered by [`DeclareChannelExt`]):
//!   a global `On<C>` observer for each declared command. On a *client* it
//!   reflect-serializes the command (symmetric with the apply path), rewrites
//!   local `Entity` refs → portable `GlobalEntityId` ([`globalize_ids_in_json`]),
//!   wraps a [`Mutation`], and pushes onto [`SyncOutbox`]. Suppressed for
//!   wire-applied commands (echo guard) and in single-player.
//! - **apply** ([`apply_sync_command`]): an `On<SyncCommandEvent>` observer that
//!   dedupes by `OpId`, authorizes (host only), resolves ids, then triggers the
//!   typed command via reflection with [`SyncApplyGuard`] set so the capture
//!   observer doesn't echo it.
//! - **ferry**: `lunco-networking` drains [`SyncOutbox`] → lightyear messages and
//!   fills [`SyncInbox`] ← lightyear messages. [`drain_sync_inbox`] turns inbox
//!   entries into command triggers / snapshot applies / handshakes.
//! - **state**: [`gather_snapshot`] (host) emits changed transforms at a tunable
//!   HZ; clients apply them in `drain_sync_inbox`. [`broadcast_new_spawns`]
//!   replicates runtime spawns with the host-allocated id.

use avian3d::prelude::{AngularVelocity, LinearVelocity, PhysicsSystems, Position, Rotation};
use big_space::prelude::CellCoord;
use bevy::ecs::reflect::ReflectEvent;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::TypePath;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use lunco_core::{
    authorize, AppliedInputSeq, GlobalEntityId, IncomingSnapshots, LocalSession, Mutation,
    NetReplicate, NetSpawn, NetworkRole, OpId, PendingReplicatedSpawns, ReplicatedSpawn, SessionId,
    SessionRegistry, SimTick, SnapshotSample, SyncApplyGuard, SyncChannel,
};

use lunco_api::executor::{authz_target_gid, globalize_command_ids, resolve_command_ids};
use lunco_api::registry::ApiEntityRegistry;

// ── Wire payloads ─────────────────────────────────────────────────────────────

/// A command on the wire: its short type name (e.g. `"DriveRover"`) + the
/// reflect-serialized params as a **JSON string**, with `Entity` refs expressed as
/// `GlobalEntityId`s. The payload is JSON *text* (not a `serde_json::Value`) so the
/// envelope round-trips through the binary `bincode` codec — bincode is not
/// self-describing and cannot deserialize a `Value` (`deserialize_any`). The id
/// translation still operates on a parsed `Value`; this is just the wire form.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncCommand {
    pub type_name: String,
    pub data: String,
}

/// One entity's replicated transform (+ velocity), keyed by [`GlobalEntityId`]
/// raw `u64`. **Compact wire form (Phase 3):** the absolute world position is
/// fixed-point quantized to **1 mm** in `i32` ([`POS_SCALE`]) and the world
/// rotation is a 32-bit smallest-three packing — replacing the old f32 `t` +
/// f32 quat + f64 `pos` + i64 `cell` (≈112 B → ≈52 B). Velocities stay f32 to
/// protect owned-rover reconcile precision.
///
/// `pos_q` spans ±(2³¹−1)/`POS_SCALE` ≈ **±2 147 km** from the world origin —
/// the whole lunar surface (radius 1 737 km) and the entire sandbox. Bodies
/// farther than that saturate at the bound (see [`quantize_pos`]); covering a
/// deep-orbital / cislunar-absolute frame needs big_space recentering, which is
/// **deferred** (blocked on an avian↔big_space transform-writeback bridge — see
/// `DESIGN_GAPS §A` and `crates/lunco-core/src/coords.rs` rebase tests).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub gid: u64,
    /// Absolute world position (avian f64 `Position`), fixed-point at
    /// [`POS_SCALE`]. Decode with [`dequantize_pos`].
    pub pos_q: [i32; 3],
    /// World-space rotation, smallest-three packed ([`encode_quat`] /
    /// [`decode_quat`]).
    pub rot_packed: u32,
    /// Authoritative linear velocity (avian `LinearVelocity`, f64→f32) — the
    /// owned-rover prediction seats the body with this for replay.
    #[serde(default)]
    pub lv: [f32; 3],
    /// Authoritative angular velocity (avian `AngularVelocity`, f64→f32).
    #[serde(default)]
    pub av: [f32; 3],
    /// Highest input `seq` the host has applied for this gid (0 = none) — the
    /// reconcile ack for the owning client.
    #[serde(default)]
    pub last_input_seq: u32,
}

/// Fixed-point scale for wire position quantization: units per metre. `1000` ⇒
/// 1 mm resolution; `i32` then spans ±(2³¹−1)/1000 ≈ ±2 147 km from origin.
pub const POS_SCALE: f64 = 1000.0;

/// Quantize an absolute world position to fixed-point `i32` at [`POS_SCALE`].
/// Rust's float→int `as` **saturates**, so a body beyond ±2 147 km clamps to the
/// bound (a visible offset, never a wrapping teleport).
pub fn quantize_pos(p: DVec3) -> [i32; 3] {
    [
        (p.x * POS_SCALE).round() as i32,
        (p.y * POS_SCALE).round() as i32,
        (p.z * POS_SCALE).round() as i32,
    ]
}

/// Inverse of [`quantize_pos`].
pub fn dequantize_pos(q: [i32; 3]) -> DVec3 {
    DVec3::new(
        q[0] as f64 / POS_SCALE,
        q[1] as f64 / POS_SCALE,
        q[2] as f64 / POS_SCALE,
    )
}

/// Pack a unit quaternion into 32 bits (smallest-three): a 2-bit largest-component
/// index + three 10-bit components over ±1/√2. `q` and `−q` encode identically
/// (the dropped largest component is reconstructed non-negative). Max component
/// error ≈ 1.4e-3 ⇒ well under the reconcile rotation tolerance.
pub fn encode_quat(q: Quat) -> u32 {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let q = q.normalize();
    let a = [q.x, q.y, q.z, q.w];
    let mut m = 0usize;
    for i in 1..4 {
        if a[i].abs() > a[m].abs() {
            m = i;
        }
    }
    let sign = if a[m] < 0.0 { -1.0 } else { 1.0 };
    let mut packed = (m as u32) << 30;
    let mut shift = 20i32;
    for (i, &c) in a.iter().enumerate() {
        if i == m {
            continue;
        }
        let cs = c * sign; // ∈ [-1/√2, 1/√2]
        let u = ((cs / INV_SQRT2) * 0.5 + 0.5).clamp(0.0, 1.0);
        packed |= ((u * 1023.0).round() as u32) << shift;
        shift -= 10;
    }
    packed
}

/// Inverse of [`encode_quat`].
pub fn decode_quat(packed: u32) -> Quat {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let m = ((packed >> 30) & 0x3) as usize;
    let mut comps = [0.0f32; 4];
    let mut shift = 20i32;
    let mut sum_sq = 0.0f32;
    for i in 0..4 {
        if i == m {
            continue;
        }
        let q10 = (packed >> shift) & 0x3FF;
        let c = (q10 as f32 / 1023.0 * 2.0 - 1.0) * INV_SQRT2;
        comps[i] = c;
        sum_sq += c * c;
        shift -= 10;
    }
    comps[m] = (1.0 - sum_sq).max(0.0).sqrt();
    Quat::from_xyzw(comps[0], comps[1], comps[2], comps[3]).normalize()
}

/// A batch of changed transforms at a given sim tick (M2 state replication).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotMsg {
    pub tick: u64,
    pub entries: Vec<SnapshotEntry>,
}

/// Host → clients: instantiate this catalog entry locally pinned to the
/// host-allocated id (M1 content-reconstruction).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpawnReplicationMsg {
    pub gid: u64,
    pub entry_id: String,
    pub position: [f32; 3],
}

/// Host → a freshly-connected client: your session id + the current tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandshakeMsg {
    pub session: u64,
    pub tick: u64,
}

/// Host → clients: the authoritative who-owns-what map (`gid → session`).
/// Replaces the client's view of [`lunco_core::SessionRegistry`] so possession
/// is exclusive and synced across peers — clients refuse to possess an
/// already-owned vessel and drop control of one they've lost. Broadcast on
/// change over the reliable CommandBus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnershipMsg {
    /// `(gid, session)` pairs — the full current ownership table.
    pub entries: Vec<(u64, u64)>,
}

/// Everything that crosses the wire, tagged for reliable/unreliable routing by
/// the accompanying [`SyncChannel`]. `lunco-networking` (de)serializes these.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncEnvelope {
    Command(Mutation<SyncCommand>),
    Snapshot(SnapshotMsg),
    Spawn(SpawnReplicationMsg),
    Handshake(HandshakeMsg),
    Ownership(OwnershipMsg),
    Ack(lunco_core::Ack),
}

// ── Resources (the contract `lunco-networking` touches) ───────────────────────

/// Outgoing envelopes awaiting ferry to the wire. Drained by `lunco-networking`;
/// stays empty when no adapter runs (single-player no-op).
#[derive(Resource, Default)]
pub struct SyncOutbox(pub Vec<(SyncChannel, SyncEnvelope)>);

/// Incoming envelopes from the wire, each tagged with the sender's session
/// (host uses this to attribute authority). Filled by `lunco-networking`.
#[derive(Resource, Default)]
pub struct SyncInbox(pub Vec<(SessionId, SyncEnvelope)>);

/// Tunable replication knobs (the user's "HZ + only-if-changed" ask).
#[derive(Resource, Clone, Debug)]
pub struct NetworkConfig {
    /// Snapshot send rate (Hz). Default 20.
    pub replication_hz: f32,
    /// Only include entities whose transform changed since the last snapshot.
    pub only_if_changed: bool,
    /// Which channel snapshots ride (default best-effort `ControlStream`).
    pub snapshot_channel: SyncChannel,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            replication_hz: 20.0,
            only_if_changed: true,
            snapshot_channel: SyncChannel::ControlStream,
        }
    }
}

/// short type name → its declared [`SyncChannel`].
#[derive(Resource, Default)]
pub struct SyncChannelRegistry(pub HashMap<String, SyncChannel>);

/// Bounded set of recently-applied `OpId`s for idempotent replay rejection.
#[derive(Resource)]
pub struct SyncDedup {
    seen: HashSet<u64>,
    order: VecDeque<u64>,
    cap: usize,
}

impl Default for SyncDedup {
    fn default() -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            cap: 8192,
        }
    }
}

impl SyncDedup {
    /// `true` if `op` is new (apply it); `false` if already seen (drop).
    pub fn check_and_insert(&mut self, op: OpId) -> bool {
        if !self.seen.insert(op.0) {
            return false;
        }
        self.order.push_back(op.0);
        if self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
}

/// Fired by [`drain_sync_inbox`] for each inbound command; handled by
/// [`apply_sync_command`].
#[derive(Event, Debug, Clone)]
pub struct SyncCommandEvent {
    pub type_name: String,
    pub params: serde_json::Value,
    pub op_id: OpId,
    pub origin: SessionId,
}

// ── Channel declaration + capture ─────────────────────────────────────────────

/// Declare which [`SyncChannel`] a command type rides, and (unless `Local`)
/// register its capture observer. Called by `lunco-networking` for each
/// networked command (e.g. `DriveRover` → `ControlStream`, `PossessVessel` →
/// `CommandBus`). No-op-on-the-wire commands need not be declared.
pub trait DeclareChannelExt {
    fn declare_channel<C: Event + Reflect + TypePath>(&mut self, channel: SyncChannel) -> &mut Self;
}

impl DeclareChannelExt for App {
    fn declare_channel<C: Event + Reflect + TypePath>(&mut self, channel: SyncChannel) -> &mut Self {
        let name = C::short_type_path().to_string();
        if !self.world().contains_resource::<SyncChannelRegistry>() {
            self.init_resource::<SyncChannelRegistry>();
        }
        self.world_mut()
            .resource_mut::<SyncChannelRegistry>()
            .0
            .insert(name, channel);
        if channel != SyncChannel::Local {
            self.add_observer(capture_command::<C>);
        }
        self
    }
}

/// Global `On<C>` observer: serialize a locally-originated command and enqueue
/// it for the wire. **Client-only** — the host is authoritative and applies its
/// own commands directly (clients learn the result via snapshots), and a
/// standalone peer has no wire.
fn capture_command<C: Event + Reflect + TypePath>(
    trigger: On<C>,
    role: Res<NetworkRole>,
    guard: Res<SyncApplyGuard>,
    local: Res<LocalSession>,
    type_registry: Res<AppTypeRegistry>,
    entity_registry: Res<ApiEntityRegistry>,
    channels: Res<SyncChannelRegistry>,
    mut outbox: ResMut<SyncOutbox>,
) {
    // Only a pure client emits commands onto the wire.
    if *role != NetworkRole::Client {
        return;
    }
    // Echo guard: this command arrived from the wire; don't re-send it.
    if guard.is_from_sync() {
        return;
    }

    let cmd = trigger.event();
    let type_name = C::short_type_path().to_string();
    let channel = channels
        .0
        .get(&type_name)
        .copied()
        .unwrap_or(SyncChannel::CommandBus);

    // Serialize through the SAME reflect path the apply side deserializes with.
    let mut data = {
        let type_reg = type_registry.read();
        let serializer = TypedReflectSerializer::new(cmd.as_partial_reflect(), &type_reg);
        match serde_json::to_value(&serializer) {
            Ok(v) => v,
            Err(e) => {
                warn!("[sync] capture serialize {type_name} failed: {e}");
                return;
            }
        }
    };
    // Local Entity refs (to_bits) → portable GlobalEntityId, driven by `C`'s
    // reflect schema. Fields tagged `#[sync_local]` (e.g. a possession's
    // `avatar`) are replaced with `Entity::PLACEHOLDER` inside the walker — a
    // local camera concern whose bits must never leak; wire control identity is
    // the session `origin`, not the avatar entity. No field-name special-casing.
    {
        let type_reg = type_registry.read();
        globalize_command_ids(
            &mut data,
            std::any::TypeId::of::<C>(),
            &type_reg,
            &entity_registry,
        );
    }

    // Serialize the (id-translated) Value to compact JSON text for the wire.
    let data = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
    let mut mutation = Mutation::local(SyncCommand { type_name, data });
    mutation.origin = local.0;
    outbox.0.push((channel, SyncEnvelope::Command(mutation)));
}

// ── Apply ─────────────────────────────────────────────────────────────────────

/// Apply an inbound command through the *same* reflect-trigger path as a local /
/// HTTP command, with dedupe + authority + an echo guard.
pub fn apply_sync_command(
    trigger: On<SyncCommandEvent>,
    mut commands: Commands,
    type_registry: Res<AppTypeRegistry>,
    session_registry: Res<SessionRegistry>,
    role: Res<NetworkRole>,
    mut dedup: ResMut<SyncDedup>,
) {
    let ev = trigger.event();
    if !dedup.check_and_insert(ev.op_id) {
        return; // duplicate (Reject::Duplicate, silently absorbed)
    }
    // Host authorizes against ownership; a client trusts the host.
    if role.is_host() {
        // The gid to authorize against is the command's `#[authz_target]`
        // field (schema-driven), read from the raw global-gid wire params —
        // no hardcoded field name.
        let target_gid = {
            let type_reg = type_registry.read();
            type_reg
                .get_with_short_type_path(&ev.type_name)
                .and_then(|r| authz_target_gid(&ev.params, r.type_id(), &type_reg))
        };
        if let Err(reject) = authorize(&session_registry, ev.origin, &ev.type_name, target_gid) {
            // Diagnostic: show the gid the command targets, who (if anyone) the
            // host thinks owns it, and the full ownership table — so a drive
            // rejected despite a successful possession reveals whether it's a
            // target/possession gid mismatch (articulated root vs clicked link)
            // or an empty/stale ownership table (registry desync).
            let owner = target_gid.and_then(|g| session_registry.owner_of(g));
            warn!(
                "[sync] rejected {} from {}: {:?} | target_gid={:?} current_owner={:?} owners={:?}",
                ev.type_name, ev.origin, reject, target_gid, owner, session_registry.snapshot(),
            );
            return;
        }
        // NOTE: the reconcile ack (highest applied input seq per gid) is no longer
        // recorded here. It now lives in the single `record_drive_input` /
        // `record_brake_input` observers (lunco-controller), which fire when the
        // command below is triggered — so the ack is recorded identically whether a
        // drive arrives over the wire, from the API, or from the local keyboard.
    }

    let mut params = ev.params.clone();
    let type_name = ev.type_name.clone();
    let origin = ev.origin;

    commands.queue(move |world: &mut World| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let type_reg = registry.read();
        let Some(registration) = type_reg.get_with_short_type_path(&type_name) else {
            warn!("[sync] unknown command type '{type_name}'");
            return;
        };
        let Some(reflect_event) = registration.data::<ReflectEvent>() else {
            warn!("[sync] command '{type_name}' has no ReflectEvent");
            return;
        };
        // Wire gids → local Entity bits, driven by the command's schema (needs
        // `registration`, hence inside the queued closure). Mirrors the capture
        // side's `globalize_command_ids`.
        {
            let entity_registry = world.resource::<ApiEntityRegistry>();
            resolve_command_ids(&mut params, registration.type_id(), &type_reg, entity_registry);
        }
        let deserializer = TypedReflectDeserializer::new(registration, &type_reg);
        use serde::de::DeserializeSeed;
        let reflected = match deserializer.deserialize(params) {
            Ok(r) => r,
            Err(e) => {
                warn!("[sync] deserialize '{type_name}' failed: {e}");
                return;
            }
        };
        // Guard against a panic in `ReflectEvent::trigger`: it builds the concrete
        // type via `FromReflect`, falling back to `Default`/`FromWorld` and
        // PANICKING when none apply (e.g. a struct still missing a no-`Default`
        // field). Verify the value is fully constructible first so a malformed
        // *wire* command logs and is dropped instead of killing the host. Mirrors
        // the same guard in `lunco-api`'s `api_command_dispatcher`. (Types without
        // a registered `ReflectFromReflect` keep the legacy path.)
        let constructible = registration
            .data::<bevy::reflect::ReflectFromReflect>()
            .map(|fr| fr.from_reflect(reflected.as_ref()).is_some())
            .unwrap_or(true);
        if !constructible {
            warn!("[sync] command '{type_name}' not constructible from params (missing/invalid fields); dropped");
            return;
        }
        // Guard set → the capture observer for this command suppresses the echo,
        // and possession skips local camera-bind for a remote origin.
        world.resource_mut::<SyncApplyGuard>().0 = Some(origin);
        reflect_event.trigger(world, reflected.as_ref(), &type_reg);
        world.resource_mut::<SyncApplyGuard>().0 = None;
    });
}

// ── Inbox drain (commands / snapshots / spawns / handshake) ───────────────────

#[allow(clippy::too_many_arguments)]
pub fn drain_sync_inbox(
    mut inbox: ResMut<SyncInbox>,
    mut commands: Commands,
    role: Res<NetworkRole>,
    mut local: ResMut<LocalSession>,
    mut tick: ResMut<SimTick>,
    mut pending_spawns: ResMut<PendingReplicatedSpawns>,
    mut snapshots: ResMut<IncomingSnapshots>,
    mut registry: ResMut<SessionRegistry>,
) {
    if inbox.0.is_empty() {
        return;
    }
    let drained: Vec<(SessionId, SyncEnvelope)> = std::mem::take(&mut inbox.0);
    for (sender, env) in drained {
        match env {
            SyncEnvelope::Command(m) => {
                // Host attributes to the connection-derived session (don't trust
                // a client-claimed origin); a client trusts the host.
                let origin = if role.is_host() { sender } else { m.origin };
                // Wire form is JSON text (bincode-safe); parse back to a `Value`
                // for the schema-driven id resolution + reflect deserialize.
                let params =
                    serde_json::from_str(&m.payload.data).unwrap_or(serde_json::Value::Null);
                commands.trigger(SyncCommandEvent {
                    type_name: m.payload.type_name,
                    params,
                    op_id: m.id,
                    origin,
                });
            }
            SyncEnvelope::Snapshot(snap) => {
                // Queue for the avian-aware apply (sets `Position` so the
                // physics transform-sync doesn't overwrite it).
                for e in snap.entries {
                    // Decode the compact wire form back to the absolute f64 the
                    // apply path expects. Cells are not used (recentering deferred),
                    // so render-space `t` == absolute and `cell` is zero.
                    let pos = dequantize_pos(e.pos_q);
                    snapshots.0.push(SnapshotSample {
                        gid: e.gid,
                        tick: snap.tick,
                        t: pos.as_vec3().to_array(),
                        r: decode_quat(e.rot_packed).to_array(),
                        lv: e.lv,
                        av: e.av,
                        last_input_seq: e.last_input_seq,
                        pos: [pos.x, pos.y, pos.z],
                        cell: [0, 0, 0],
                    });
                }
                tick.0 = snap.tick;
            }
            SyncEnvelope::Spawn(s) => {
                pending_spawns.0.push(ReplicatedSpawn {
                    gid: s.gid,
                    entry_id: s.entry_id,
                    position: Vec3::from_array(s.position),
                });
            }
            SyncEnvelope::Handshake(h) => {
                local.0 = SessionId(h.session);
                tick.0 = h.tick;
                info!("[net] handshake: session={} tick={}", h.session, h.tick);
            }
            SyncEnvelope::Ownership(o) => {
                // Clients adopt the host's authoritative who-owns-what table.
                // (The host never receives this — it *is* the authority.)
                if !role.is_host() {
                    registry.replace_all(o.entries.into_iter().map(|(g, s)| (g, SessionId(s))));
                }
            }
            SyncEnvelope::Ack(_) => { /* MVP is optimistic; acks unused */ }
        }
    }
}

// ── State replication (host → clients) ────────────────────────────────────────

/// Host: at the configured HZ, emit a snapshot of changed networked transforms.
pub fn gather_snapshot(
    role: Res<NetworkRole>,
    config: Res<NetworkConfig>,
    time: Res<Time>,
    tick: Res<SimTick>,
    mut acc: Local<f32>,
    mut last_sent: Local<HashMap<u64, ([i32; 3], u32)>>,
    applied: Res<AppliedInputSeq>,
    q: Query<
        (
            &GlobalEntityId,
            &Transform,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            Option<&Position>,
            Option<&Rotation>,
            Option<&CellCoord>,
        ),
        With<NetReplicate>,
    >,
    mut outbox: ResMut<SyncOutbox>,
) {
    if !role.is_host() {
        return;
    }
    *acc += time.delta_secs();
    let interval = 1.0 / config.replication_hz.max(1.0);
    if *acc < interval {
        return;
    }
    *acc = 0.0;

    let mut entries = Vec::new();
    for (gid, tf, lin, ang, position, rotation, _cell) in q.iter() {
        let key = gid.get();
        // Rotation on the wire is WORLD-space: prefer avian's world `Rotation`,
        // fall back to `Transform.rotation` for bodies without a physics Rotation.
        // For a top-level body the two are identical, so existing replicated
        // bodies are unaffected; for a NESTED body (an articulated rover's wheel)
        // `Transform.rotation` is parent-LOCAL while the client applies it to
        // world `Rotation` — only the avian-world value reconstructs correctly.
        // (`pos` below already prefers avian world `Position` for the same reason.)
        let rot = rotation.map(|r| r.0.as_quat()).unwrap_or(tf.rotation);
        // Absolute world position: prefer the precise avian f64 `Position`; fall
        // back to the f32 `Transform` (as f64) for bodies without a physics Position.
        // Cells are not carried (recentering deferred — see `SnapshotEntry` doc), so
        // `pos` is the full absolute world position.
        let pos = position.map(|p| p.0).unwrap_or_else(|| {
            DVec3::new(
                tf.translation.x as f64,
                tf.translation.y as f64,
                tf.translation.z as f64,
            )
        });
        // Quantize to the compact wire form once and change-detect on THAT form,
        // so sub-quantum jitter never triggers a resend.
        let pos_q = quantize_pos(pos);
        let rot_packed = encode_quat(rot);
        // only_if_changed gates on POSE — velocity rides along when the pose
        // changes (at rest the body emits nothing and the client holds the last
        // sample, whose velocity is ~0 by then). Good enough for prediction.
        if config.only_if_changed {
            if let Some((lp, lr)) = last_sent.get(&key) {
                if *lp == pos_q && *lr == rot_packed {
                    continue;
                }
            }
        }
        last_sent.insert(key, (pos_q, rot_packed));
        let lv = lin.map(|v| v.0.as_vec3().to_array()).unwrap_or([0.0; 3]);
        let av = ang.map(|v| v.0.as_vec3().to_array()).unwrap_or([0.0; 3]);
        entries.push(SnapshotEntry {
            gid: key,
            pos_q,
            rot_packed,
            lv,
            av,
            last_input_seq: applied.0.get(&key).copied().unwrap_or(0),
        });
    }
    if entries.is_empty() {
        return;
    }
    outbox.0.push((
        config.snapshot_channel,
        SyncEnvelope::Snapshot(SnapshotMsg {
            tick: tick.0,
            entries,
        }),
    ));
}

/// Host: when a runtime-spawned networked root gets its id minted, replicate the
/// spawn to clients so they reconstruct the geometry locally pinned to that id.
pub fn broadcast_new_spawns(
    role: Res<NetworkRole>,
    q: Query<(&GlobalEntityId, &NetSpawn), Added<GlobalEntityId>>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if !role.is_host() {
        return;
    }
    for (gid, spawn) in q.iter() {
        outbox.0.push((
            SyncChannel::CommandBus,
            SyncEnvelope::Spawn(SpawnReplicationMsg {
                gid: gid.get(),
                entry_id: spawn.entry_id.clone(),
                position: spawn.position.to_array(),
            }),
        ));
    }
}

// ── Plugin (always-on; inert in single-player) ────────────────────────────────

/// Registers the wire substrate. Added unconditionally by `LunCoApiPlugin`; all
/// its systems early-return under [`NetworkRole::Standalone`], so single-player
/// pays nothing.
pub struct SyncPlugin;

impl Plugin for SyncPlugin {
    fn build(&self, app: &mut App) {
        // CONVENTION: SyncPlugin initializes ONLY wire-only state (envelope
        // queues, dedup, transport/replication config). Always-on substrate
        // resources — anything read by systems that run even with networking
        // off (e.g. AppliedInputSeq / OwnedInputLog) — belong in
        // LunCoCorePlugin (lunco-core), never here. SyncPlugin is behind the
        // `networking` feature, so initializing substrate here panics
        // single-player builds.
        app.init_resource::<SyncOutbox>()
            .init_resource::<SyncInbox>()
            .init_resource::<SyncDedup>()
            .init_resource::<NetworkConfig>()
            .init_resource::<SyncChannelRegistry>()
            .add_observer(apply_sync_command)
            // `drain_sync_inbox` + `broadcast_new_spawns` stay in `Update` alongside
            // the lightyear ferry (see server.rs note: FixedUpdate breaks the reliable
            // CmdChannel — it does NOT touch lightyear, but it consumes what the ferry
            // produced this frame and feeds the ferry's send, so it shares the ferry's
            // schedule).
            .add_systems(Update, (drain_sync_inbox, broadcast_new_spawns))
            // `gather_snapshot` runs on the sim clock (`FixedPostUpdate`): it only
            // writes our `SyncOutbox` (never calls lightyear), so it's safe off the
            // render clock. This decouples snapshot GENERATION (a steady 20 Hz,
            // tick-stamped, even when the window is unfocused and `Update` is
            // render-throttled to ~5 Hz) from snapshot SEND (the ferry, still
            // `Update`). The ferry then drains several queued snapshots in one
            // throttled frame — a burst — but each carries its host `SimTick`, so
            // the client interpolates them in tick-space and motion stays smooth
            // (see `interpolate_proxies`).
            // `.after(Writeback)`: sample the pose AFTER avian has integrated this
            // tick and synced `Position`/`Rotation` → `Transform`, so the snapshot's
            // pose and its `last_input_seq` ack are a consistent post-step pair —
            // matching how the client records its predicted pose (`record_predicted_state`,
            // also after writeback). Otherwise the host could ship last-tick's pose
            // stamped with this-tick's ack (a 1-tick mispair the client reconciles away).
            // Schedule = `FixedPostUpdate`, NOT `FixedUpdate`: avian's
            // `PhysicsSystems::Writeback` set runs in `FixedPostUpdate`, so an
            // `.after(Writeback)` in `FixedUpdate` constrains nothing (zero set
            // members there → silent no-op) and would sample last-tick's pose
            // stamped with this-tick's `last_input_seq`. `FixedPostUpdate` is on
            // the same fixed clock (keeps the steady-20 Hz decoupling) AND honors
            // the ordering — matching the client's `record_predicted_state`,
            // which also records after writeback in `FixedPostUpdate`.
            .add_systems(FixedPostUpdate, gather_snapshot.after(PhysicsSystems::Writeback));
    }
}

#[cfg(test)]
mod codec_roundtrip {
    use super::*;
    use crate::shared::{deserialize_env, serialize_env};

    #[test]
    fn snapshot_envelope_roundtrips_through_bincode() {
        let pos_q = quantize_pos(DVec3::new(1.0, 2.0, 3.0));
        let rot_packed = encode_quat(Quat::IDENTITY);
        let env = SyncEnvelope::Snapshot(SnapshotMsg {
            tick: 42,
            entries: vec![SnapshotEntry {
                gid: 7,
                pos_q,
                rot_packed,
                lv: [0.1, 0.0, -0.2],
                av: [0.0, 0.5, 0.0],
                last_input_seq: 99,
            }],
        });
        let bytes = serialize_env(&env).expect("serialize");
        let back = deserialize_env(&bytes).expect("deserialize");
        match back {
            SyncEnvelope::Snapshot(s) => {
                assert_eq!(s.tick, 42);
                assert_eq!(s.entries.len(), 1);
                assert_eq!(s.entries[0].gid, 7);
                assert_eq!(s.entries[0].last_input_seq, 99);
                assert_eq!(s.entries[0].pos_q, pos_q);
                assert_eq!(s.entries[0].rot_packed, rot_packed);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn command_envelope_with_string_payload_roundtrips() {
        // Guards the bincode gotcha: `SyncCommand.data` must be a JSON *String*, not a
        // `serde_json::Value` — bincode is not self-describing and cannot deserialize a
        // `Value` (`deserialize_any`), which would silently drop every command
        // (possession/drive) over the wire while snapshots kept working.
        let payload = r#"{"forward":1.0,"steer":-0.5}"#;
        let env = SyncEnvelope::Command(Mutation::local(SyncCommand {
            type_name: "DriveRover".to_string(),
            data: payload.to_string(),
        }));
        let bytes = serialize_env(&env).expect("serialize");
        let back = deserialize_env(&bytes).expect("deserialize");
        match back {
            SyncEnvelope::Command(m) => {
                assert_eq!(m.payload.type_name, "DriveRover");
                assert_eq!(m.payload.data, payload);
                // The apply path re-parses the text to a Value; confirm it still does.
                let v: serde_json::Value = serde_json::from_str(&m.payload.data).unwrap();
                assert_eq!(v["forward"], 1.0);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }
}
