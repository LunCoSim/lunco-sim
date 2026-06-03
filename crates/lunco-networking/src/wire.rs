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
//!   wraps a [`Mutation`], and pushes onto [`WireOutbox`]. Suppressed for
//!   wire-applied commands (echo guard) and in single-player.
//! - **apply** ([`apply_wire_command`]): an `On<WireCommandEvent>` observer that
//!   dedupes by `OpId`, authorizes (host only), resolves ids, then triggers the
//!   typed command via reflection with [`WireApplyGuard`] set so the capture
//!   observer doesn't echo it.
//! - **ferry**: `lunco-networking` drains [`WireOutbox`] → lightyear messages and
//!   fills [`WireInbox`] ← lightyear messages. [`drain_wire_inbox`] turns inbox
//!   entries into command triggers / snapshot applies / handshakes.
//! - **state**: [`gather_snapshot`] (host) emits changed transforms at a tunable
//!   HZ; clients apply them in `drain_wire_inbox`. [`broadcast_new_spawns`]
//!   replicates runtime spawns with the host-allocated id.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position};
use big_space::prelude::CellCoord;
use bevy::ecs::reflect::ReflectEvent;
use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::TypePath;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use lunco_core::{
    authorize, AppliedInputSeq, GlobalEntityId, IncomingSnapshots, LocalSession, Mutation,
    NetReplicate, NetSpawn, NetworkRole, OpId, PendingReplicatedSpawns, ReplicatedSpawn, SessionId,
    SessionRegistry, SimTick, SnapshotSample, WireApplyGuard, WireChannel,
};

use lunco_api::executor::{globalize_ids_in_json, resolve_ids_in_json};
use lunco_api::registry::ApiEntityRegistry;

// ── Wire payloads ─────────────────────────────────────────────────────────────

/// A command on the wire: its short type name (e.g. `"DriveRover"`) + the
/// reflect-serialized params, with `Entity` refs expressed as `GlobalEntityId`s.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireCommand {
    pub type_name: String,
    pub data: serde_json::Value,
}

/// One entity's replicated transform (+ velocity), keyed by [`GlobalEntityId`]
/// raw `u64`. `lv`/`av` are `#[serde(default)]` so the wire stays
/// forward/backward-compatible (an old peer omits them → zero).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub gid: u64,
    pub t: [f32; 3],
    pub r: [f32; 4],
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
    /// Authoritative **absolute** position as avian f64 `Position` (gap A). The
    /// f32 `t` above is the render-space (cell-relative) offset and loses
    /// precision far from origin; `pos` is the precise physics truth used to seat
    /// replicated proxies at lunar/orbital scale. `#[serde(default)]` → an old
    /// peer omits it and the apply path falls back to `t`.
    #[serde(default)]
    pub pos: [f64; 3],
    /// big_space `CellCoord` of the body (i64 per axis). `[0,0,0]` in the current
    /// single-cell config (`switching_threshold = 1e10`, bodies never recenter);
    /// carried so replication stays correct once recentering is enabled.
    #[serde(default)]
    pub cell: [i64; 3],
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
/// the accompanying [`WireChannel`]. `lunco-networking` (de)serializes these.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WireEnvelope {
    Command(Mutation<WireCommand>),
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
pub struct WireOutbox(pub Vec<(WireChannel, WireEnvelope)>);

/// Incoming envelopes from the wire, each tagged with the sender's session
/// (host uses this to attribute authority). Filled by `lunco-networking`.
#[derive(Resource, Default)]
pub struct WireInbox(pub Vec<(SessionId, WireEnvelope)>);

/// Tunable replication knobs (the user's "HZ + only-if-changed" ask).
#[derive(Resource, Clone, Debug)]
pub struct NetworkConfig {
    /// Snapshot send rate (Hz). Default 20.
    pub replication_hz: f32,
    /// Only include entities whose transform changed since the last snapshot.
    pub only_if_changed: bool,
    /// Which channel snapshots ride (default best-effort `ControlStream`).
    pub snapshot_channel: WireChannel,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            replication_hz: 20.0,
            only_if_changed: true,
            snapshot_channel: WireChannel::ControlStream,
        }
    }
}

/// short type name → its declared [`WireChannel`].
#[derive(Resource, Default)]
pub struct WireChannelRegistry(pub HashMap<String, WireChannel>);

/// Bounded set of recently-applied `OpId`s for idempotent replay rejection.
#[derive(Resource)]
pub struct WireDedup {
    seen: HashSet<u64>,
    order: VecDeque<u64>,
    cap: usize,
}

impl Default for WireDedup {
    fn default() -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            cap: 8192,
        }
    }
}

impl WireDedup {
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

/// Fired by [`drain_wire_inbox`] for each inbound command; handled by
/// [`apply_wire_command`].
#[derive(Event, Debug, Clone)]
pub struct WireCommandEvent {
    pub type_name: String,
    pub params: serde_json::Value,
    pub op_id: OpId,
    pub origin: SessionId,
}

// ── Channel declaration + capture ─────────────────────────────────────────────

/// Declare which [`WireChannel`] a command type rides, and (unless `Local`)
/// register its capture observer. Called by `lunco-networking` for each
/// networked command (e.g. `DriveRover` → `ControlStream`, `PossessVessel` →
/// `CommandBus`). No-op-on-the-wire commands need not be declared.
pub trait DeclareChannelExt {
    fn declare_channel<C: Event + Reflect + TypePath>(&mut self, channel: WireChannel) -> &mut Self;
}

impl DeclareChannelExt for App {
    fn declare_channel<C: Event + Reflect + TypePath>(&mut self, channel: WireChannel) -> &mut Self {
        let name = C::short_type_path().to_string();
        if !self.world().contains_resource::<WireChannelRegistry>() {
            self.init_resource::<WireChannelRegistry>();
        }
        self.world_mut()
            .resource_mut::<WireChannelRegistry>()
            .0
            .insert(name, channel);
        if channel != WireChannel::Local {
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
    guard: Res<WireApplyGuard>,
    local: Res<LocalSession>,
    type_registry: Res<AppTypeRegistry>,
    entity_registry: Res<ApiEntityRegistry>,
    channels: Res<WireChannelRegistry>,
    mut outbox: ResMut<WireOutbox>,
) {
    // Only a pure client emits commands onto the wire.
    if *role != NetworkRole::Client {
        return;
    }
    // Echo guard: this command arrived from the wire; don't re-send it.
    if guard.is_from_wire() {
        return;
    }

    let cmd = trigger.event();
    let type_name = C::short_type_path().to_string();
    let channel = channels
        .0
        .get(&type_name)
        .copied()
        .unwrap_or(WireChannel::CommandBus);

    // Serialize through the SAME reflect path the apply side deserializes with.
    let mut data = {
        let type_reg = type_registry.read();
        let serializer = TypedReflectSerializer::new(cmd.as_partial_reflect(), &type_reg);
        match serde_json::to_value(&serializer) {
            Ok(v) => v,
            Err(e) => {
                warn!("[wire] capture serialize {type_name} failed: {e}");
                return;
            }
        }
    };
    // Local Entity refs (to_bits) → portable GlobalEntityId.
    globalize_ids_in_json(&mut data, &entity_registry);
    // The avatar is a *local* camera concern — control identity on the wire is
    // the session (`origin`), never the avatar entity. Strip it so we don't leak
    // this peer's local entity bits: the receiver ignores it anyway (the host
    // records authority by `origin`, and a wire-applied possession skips the
    // local camera-bind). A placeholder keeps the field reflect-deserializable.
    if let Some(av) = data.get_mut("avatar") {
        *av = serde_json::json!(Entity::PLACEHOLDER.to_bits());
    }

    let mut mutation = Mutation::local(WireCommand { type_name, data });
    mutation.origin = local.0;
    outbox.0.push((channel, WireEnvelope::Command(mutation)));
}

// ── Apply ─────────────────────────────────────────────────────────────────────

fn extract_target_gid(params: &serde_json::Value) -> Option<u64> {
    params.get("target").and_then(|v| v.as_u64())
}

/// Apply an inbound command through the *same* reflect-trigger path as a local /
/// HTTP command, with dedupe + authority + an echo guard.
pub fn apply_wire_command(
    trigger: On<WireCommandEvent>,
    mut commands: Commands,
    entity_registry: Res<ApiEntityRegistry>,
    session_registry: Res<SessionRegistry>,
    role: Res<NetworkRole>,
    mut dedup: ResMut<WireDedup>,
) {
    let ev = trigger.event();
    if !dedup.check_and_insert(ev.op_id) {
        return; // duplicate (Reject::Duplicate, silently absorbed)
    }
    // Host authorizes against ownership; a client trusts the host.
    if role.is_host() {
        let target_gid = extract_target_gid(&ev.params);
        if let Err(reject) = authorize(&session_registry, ev.origin, &ev.type_name, target_gid) {
            warn!(
                "[wire] rejected {} from {}: {:?}",
                ev.type_name, ev.origin, reject
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
    resolve_ids_in_json(&mut params, &entity_registry);
    let type_name = ev.type_name.clone();
    let origin = ev.origin;

    commands.queue(move |world: &mut World| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let type_reg = registry.read();
        let Some(registration) = type_reg.get_with_short_type_path(&type_name) else {
            warn!("[wire] unknown command type '{type_name}'");
            return;
        };
        let Some(reflect_event) = registration.data::<ReflectEvent>() else {
            warn!("[wire] command '{type_name}' has no ReflectEvent");
            return;
        };
        let deserializer = TypedReflectDeserializer::new(registration, &type_reg);
        use serde::de::DeserializeSeed;
        let reflected = match deserializer.deserialize(params) {
            Ok(r) => r,
            Err(e) => {
                warn!("[wire] deserialize '{type_name}' failed: {e}");
                return;
            }
        };
        // Guard set → the capture observer for this command suppresses the echo,
        // and possession skips local camera-bind for a remote origin.
        world.resource_mut::<WireApplyGuard>().0 = Some(origin);
        reflect_event.trigger(world, reflected.as_ref(), &type_reg);
        world.resource_mut::<WireApplyGuard>().0 = None;
    });
}

// ── Inbox drain (commands / snapshots / spawns / handshake) ───────────────────

#[allow(clippy::too_many_arguments)]
pub fn drain_wire_inbox(
    mut inbox: ResMut<WireInbox>,
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
    let drained: Vec<(SessionId, WireEnvelope)> = std::mem::take(&mut inbox.0);
    for (sender, env) in drained {
        match env {
            WireEnvelope::Command(m) => {
                // Host attributes to the connection-derived session (don't trust
                // a client-claimed origin); a client trusts the host.
                let origin = if role.is_host() { sender } else { m.origin };
                commands.trigger(WireCommandEvent {
                    type_name: m.payload.type_name,
                    params: m.payload.data,
                    op_id: m.id,
                    origin,
                });
            }
            WireEnvelope::Snapshot(snap) => {
                // Queue for the avian-aware apply (sets `Position` so the
                // physics transform-sync doesn't overwrite it).
                for e in snap.entries {
                    snapshots.0.push(SnapshotSample {
                        gid: e.gid,
                        tick: snap.tick,
                        t: e.t,
                        r: e.r,
                        lv: e.lv,
                        av: e.av,
                        last_input_seq: e.last_input_seq,
                        pos: e.pos,
                        cell: e.cell,
                    });
                }
                tick.0 = snap.tick;
            }
            WireEnvelope::Spawn(s) => {
                pending_spawns.0.push(ReplicatedSpawn {
                    gid: s.gid,
                    entry_id: s.entry_id,
                    position: Vec3::from_array(s.position),
                });
            }
            WireEnvelope::Handshake(h) => {
                local.0 = SessionId(h.session);
                tick.0 = h.tick;
                info!("[wire] handshake: session={} tick={}", h.session, h.tick);
            }
            WireEnvelope::Ownership(o) => {
                // Clients adopt the host's authoritative who-owns-what table.
                // (The host never receives this — it *is* the authority.)
                if !role.is_host() {
                    registry.replace_all(o.entries.into_iter().map(|(g, s)| (g, SessionId(s))));
                }
            }
            WireEnvelope::Ack(_) => { /* MVP is optimistic; acks unused */ }
        }
    }
}

// ── State replication (host → clients) ────────────────────────────────────────

fn arr3_eq(a: &[f32; 3], b: &[f32; 3]) -> bool {
    const E: f32 = 1e-4;
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < E)
}

fn arr4_eq(a: &[f32; 4], b: &[f32; 4]) -> bool {
    const E: f32 = 1e-4;
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < E)
}

/// Host: at the configured HZ, emit a snapshot of changed networked transforms.
pub fn gather_snapshot(
    role: Res<NetworkRole>,
    config: Res<NetworkConfig>,
    time: Res<Time>,
    tick: Res<SimTick>,
    mut acc: Local<f32>,
    mut last_sent: Local<HashMap<u64, ([f32; 3], [f32; 4])>>,
    applied: Res<AppliedInputSeq>,
    q: Query<
        (
            &GlobalEntityId,
            &Transform,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            Option<&Position>,
            Option<&CellCoord>,
        ),
        With<NetReplicate>,
    >,
    mut outbox: ResMut<WireOutbox>,
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
    for (gid, tf, lin, ang, position, cell) in q.iter() {
        let key = gid.get();
        let t = tf.translation.to_array();
        let r = tf.rotation.to_array();
        // only_if_changed gates on POSE — velocity rides along when the pose
        // changes (at rest the body emits nothing and the client holds the last
        // sample, whose velocity is ~0 by then). Good enough for prediction.
        if config.only_if_changed {
            if let Some((lt, lr)) = last_sent.get(&key) {
                if arr3_eq(lt, &t) && arr4_eq(lr, &r) {
                    continue;
                }
            }
        }
        last_sent.insert(key, (t, r));
        let lv = lin.map(|v| v.0.as_vec3().to_array()).unwrap_or([0.0; 3]);
        let av = ang.map(|v| v.0.as_vec3().to_array()).unwrap_or([0.0; 3]);
        // Absolute position: prefer the precise avian f64 `Position`; fall back to
        // the f32 `Transform` (as f64) for bodies without a physics Position.
        let pos = position
            .map(|p| [p.0.x, p.0.y, p.0.z])
            .unwrap_or([t[0] as f64, t[1] as f64, t[2] as f64]);
        let cell = cell.map(|c| [c.x as i64, c.y as i64, c.z as i64]).unwrap_or([0; 3]);
        entries.push(SnapshotEntry {
            gid: key,
            t,
            r,
            lv,
            av,
            last_input_seq: applied.0.get(&key).copied().unwrap_or(0),
            pos,
            cell,
        });
    }
    if entries.is_empty() {
        return;
    }
    outbox.0.push((
        config.snapshot_channel,
        WireEnvelope::Snapshot(SnapshotMsg {
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
    mut outbox: ResMut<WireOutbox>,
) {
    if !role.is_host() {
        return;
    }
    for (gid, spawn) in q.iter() {
        outbox.0.push((
            WireChannel::CommandBus,
            WireEnvelope::Spawn(SpawnReplicationMsg {
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
pub struct WirePlugin;

impl Plugin for WirePlugin {
    fn build(&self, app: &mut App) {
        // CONVENTION: WirePlugin initializes ONLY wire-only state (envelope
        // queues, dedup, transport/replication config). Always-on substrate
        // resources — anything read by systems that run even with networking
        // off (e.g. AppliedInputSeq / OwnedInputLog) — belong in
        // LunCoCorePlugin (lunco-core), never here. WirePlugin is behind the
        // `networking` feature, so initializing substrate here panics
        // single-player builds.
        app.init_resource::<WireOutbox>()
            .init_resource::<WireInbox>()
            .init_resource::<WireDedup>()
            .init_resource::<NetworkConfig>()
            .init_resource::<WireChannelRegistry>()
            .add_observer(apply_wire_command)
            // `drain_wire_inbox` + `broadcast_new_spawns` stay in `Update` alongside
            // the lightyear ferry (see server.rs note: FixedUpdate breaks the reliable
            // CmdChannel — it does NOT touch lightyear, but it consumes what the ferry
            // produced this frame and feeds the ferry's send, so it shares the ferry's
            // schedule).
            .add_systems(Update, (drain_wire_inbox, broadcast_new_spawns))
            // `gather_snapshot` moves to `FixedUpdate`: it only writes our `WireOutbox`
            // (never calls lightyear), so it's safe to run on the sim clock. This
            // decouples snapshot GENERATION (now a steady 20 Hz, tick-stamped, even
            // when the window is unfocused and `Update` is render-throttled to ~5 Hz)
            // from snapshot SEND (the ferry, still `Update`). The ferry then drains
            // several queued snapshots in one throttled frame — a burst — but each
            // carries its host `SimTick`, so the client interpolates them in tick-space
            // and motion stays smooth (see `interpolate_proxies`).
            .add_systems(FixedUpdate, gather_snapshot);
    }
}
