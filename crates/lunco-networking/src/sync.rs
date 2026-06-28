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

use lunco_doc::DocumentId;
use leafwing_input_manager::prelude::ActionState;
use lunco_core::{
    authorize, AppliedInputSeq, GlobalEntityId, IncomingSnapshots, LocalAvatar, LocalSession, Mutation,
    NetReplicate, NetSpawn, NetworkRole, OpId, PendingReplicatedSpawns, ReplicatedSpawn, SessionId,
    SessionRegistry, SessionProfiles, SimTick, SnapshotSample, SyncApplyGuard, SyncChannel,
};

use lunco_api::executor::{authz_target_gid, globalize_command_ids, resolve_command_ids};
use lunco_api::registry::ApiEntityRegistry;
pub use lunco_doc_bevy::{Presence, UserId, PresenceInfo};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_celestial::CelestialReferenceFrame;

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

/// Host → clients: the authoritative mapping of session ID to username and color.
/// Broadcast on change over the reliable CommandBus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfilesMsg {
    pub entries: Vec<(u64, String, [u8; 3])>,
}

/// Host/client update for mouse cursor positions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CursorUpdateMsg {
    pub session: u64,
    pub cursor: Option<[f32; 2]>,
    pub color: Option<[u8; 3]>,
}

/// Persisted user cursor sharing/update preferences.
#[derive(Resource, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct CursorSettings {
    /// Update frequency/rate in Hz.
    pub update_hz: f32,
    /// Send updates only if the cursor moved by more than a delta.
    pub update_on_delta_only: bool,
    /// Whether to broadcast/transmit local mouse cursor to peers.
    pub enabled: bool,
    /// User selected cursor / name tag color.
    pub color: [u8; 3],
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            update_hz: 10.0,
            update_on_delta_only: true,
            enabled: false,
            color: [137, 220, 235], // default sky blue color
        }
    }
}

impl SettingsSection for CursorSettings {
    const KEY: &'static str = "presence_cursor";
}

/// Host/client tutorial state update message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TutorStatusMsg {
    /// The session ID of the tutor.
    pub tutor_session: u64,
    /// The active document on the tutor's screen.
    pub active_doc: Option<DocumentId>,
    /// The active perspective on the tutor's screen.
    pub active_perspective: Option<String>,
    /// The tutor avatar's position ([f32; 3]), rotation ([f32; 4]), cell coordinates ([i64; 3]), and ephemeris ID.
    pub avatar_state: Option<([f32; 3], [f32; 4], [i64; 3], Option<i32>)>,
    /// The target client ID, if any (None = Everyone).
    pub target_client: Option<u64>,
    /// Whether the tutor is observing the target's view.
    pub observe_mode: bool,
    /// Whether the tutor allows students/followers to move freely (disables continuous follow lock).
    pub allow_free_movement: bool,
}

/// Student → tutor: streaming student's status back to tutor for observation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StudentStatusMsg {
    /// The session ID of the student.
    pub student_session: u64,
    /// The active document on the student's screen.
    pub active_doc: Option<DocumentId>,
    /// The active perspective on the student's screen.
    pub active_perspective: Option<String>,
    /// The student avatar's position ([f32; 3]), rotation ([f32; 4]), cell coordinates ([i64; 3]), and ephemeris ID.
    pub avatar_state: Option<([f32; 3], [f32; 4], [i64; 3], Option<i32>)>,
}

/// Tutor → clients: one-shot perspective sharing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharePerspectiveMsg {
    /// The session ID of the tutor.
    pub tutor_session: u64,
    /// The active document on the tutor's screen.
    pub active_doc: Option<DocumentId>,
    /// The active perspective on the tutor's screen.
    pub active_perspective: Option<String>,
    /// The tutor avatar's position ([f32; 3]), rotation ([f32; 4]), cell coordinates ([i64; 3]), and ephemeris ID.
    pub avatar_state: Option<([f32; 3], [f32; 4], [i64; 3], Option<i32>)>,
}

/// Persisted user tutorial preferences.
/// Live tutor/student session state. Although registered as a `SettingsSection`
/// for resource wiring, ALL fields are transient session state — they must NOT
/// persist across boots (you should never boot already teaching/following, with
/// a stale target, or with a sticky free-movement choice). Every field is
/// `#[serde(skip)]`, so a saved config always reloads as `Default`.
#[derive(Resource, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct TutorialSettings {
    /// Whether the local user is running as the tutor (streaming state).
    #[serde(skip)]
    pub teach_mode: bool,
    /// Whether the local user is following the tutorial (blocking input, mirroring tutor).
    #[serde(skip)]
    pub follow_mode: bool,
    /// Specific student session ID that the tutor is targeting (None = Everyone).
    #[serde(skip)]
    pub target_client: Option<u64>,
    /// Whether the tutor is currently observing the target student's view instead of broadcasting.
    #[serde(skip)]
    pub observe_mode: bool,
    /// Whether the tutor allows students/followers to move freely (disables continuous follow lock).
    #[serde(skip)]
    pub allow_free_movement: bool,
}

impl Default for TutorialSettings {
    fn default() -> Self {
        Self {
            teach_mode: false,
            follow_mode: false,
            target_client: None,
            observe_mode: false,
            // Default to LOCKED follow: when a tutor activates teach mode,
            // targeted clients auto-enter follow mode (`follow_mode =
            // !allow_free_movement`). The tutor can opt into free movement.
            allow_free_movement: false,
        }
    }
}

impl SettingsSection for TutorialSettings {
    const KEY: &'static str = "presence_tutorial";
}

/// Local cached tutor status received from the network.
#[derive(Resource, Default, Clone, Debug)]
pub struct TutorStatusResource {
    pub active_doc: Option<DocumentId>,
    pub active_perspective: Option<String>,
    pub avatar_state: Option<(Vec3, Quat, CellCoord, Option<i32>)>,
    pub target_client: Option<u64>,
    pub observe_mode: bool,
    pub allow_free_movement: bool,
    pub observed_student_doc: Option<DocumentId>,
    pub observed_student_perspective: Option<String>,
    pub observed_student_avatar_state: Option<(Vec3, Quat, CellCoord, Option<i32>)>,
    pub tutor_active: bool,
    pub last_received_time: Option<f64>,
    pub one_shot_snap_request: Option<SharePerspectiveMsg>,
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
    Profiles(ProfilesMsg),
    Ack(lunco_core::Ack),
    Cursor(CursorUpdateMsg),
    TutorStatus(TutorStatusMsg),
    StudentStatus(StudentStatusMsg),
    SharePerspective(SharePerspectiveMsg),
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

/// Bounded set of recently-applied `(origin, OpId)` pairs for idempotent replay
/// rejection. Keying on `origin` (the source [`SessionId`]) as well as the raw
/// [`OpId`] is load-bearing: two distinct processes can legitimately mint the
/// same `OpId` value (the id generator is only disjoint *across* processes, not
/// globally collision-proof), so keying on `OpId` alone would drop a real
/// command from client B as a "duplicate" of client A's — silent data loss.
#[derive(Resource)]
pub struct SyncDedup {
    seen: HashSet<(u64, u64)>,
    order: VecDeque<(u64, u64)>,
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
    /// `true` if `(origin, op)` is new (apply it); `false` if already seen
    /// (drop). The same `op` from two different origins is **not** a duplicate.
    pub fn check_and_insert(&mut self, origin: SessionId, op: OpId) -> bool {
        let key = (origin.0, op.0);
        if !self.seen.insert(key) {
            return false;
        }
        self.order.push_back(key);
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

/// Short type-paths of the state-producing control commands that the authority
/// gate ([`authorize`]) requires ownership for. The inbox drain sinks these to
/// the end of a frame's batch so a possession that arrives alongside them is
/// recorded first (see [`drain_sync_inbox`]).
fn is_control_command(type_name: &str) -> bool {
    matches!(type_name, "DriveRover" | "BrakeRover")
}

/// Apply an inbound command through the *same* reflect-trigger path as a local /
/// HTTP command, with dedupe + authority + an echo guard.
pub fn apply_sync_command(
    trigger: On<SyncCommandEvent>,
    mut commands: Commands,
    type_registry: Res<AppTypeRegistry>,
    session_registry: Res<SessionRegistry>,
    rbac: Res<lunco_core::session::SessionRbac>,
    role: Res<NetworkRole>,
    mut dedup: ResMut<SyncDedup>,
) {
    let ev = trigger.event();
    if !dedup.check_and_insert(ev.origin, ev.op_id) {
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
        if let Err(reject) = authorize(&session_registry, &rbac, ev.origin, &ev.type_name, target_gid) {
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
    mut profiles: ResMut<SessionProfiles>,
    mut presence: ResMut<Presence>,
    mut outbox: ResMut<SyncOutbox>,
    mut tutor_status: ResMut<TutorStatusResource>,
    mut tutorial_settings: ResMut<TutorialSettings>,
    time: Res<Time>,
) {
    if inbox.0.is_empty() {
        return;
    }
    let mut drained: Vec<(SessionId, SyncEnvelope)> = std::mem::take(&mut inbox.0);
    // Order within a frame: possession/structural commands BEFORE control commands.
    drained.sort_by_key(|(_, env)| match env {
        SyncEnvelope::Command(m) if is_control_command(&m.payload.type_name) => 1u8,
        _ => 0,
    });
    for (sender, env) in drained {
        match env {
            SyncEnvelope::Command(m) => {
                let params =
                    serde_json::from_str(&m.payload.data).unwrap_or(serde_json::Value::Null);
                commands.trigger(SyncCommandEvent {
                    op_id: m.id,
                    origin: sender,
                    type_name: m.payload.type_name,
                    params,
                });
            }
            SyncEnvelope::Snapshot(s) => {
                // Adopt host simulation clock (keeps remote interpolation in sync).
                if !role.is_host() {
                    let host_tick = s.tick;
                    if tick.0 < host_tick {
                        tick.0 = host_tick;
                    }
                }
                for entry in s.entries {
                    // Dequantize once; reuse for both the f32 render-space `t`
                    // and the f64 world-space `pos`. In single-cell space they are
                    // identical (cell origin is world origin).
                    // IMPORTANT: `t` must NOT be zero — `reconcile_owned_prediction`
                    // passes `sample.t` as the authority position to `reconcile_decision`
                    // for the apples-to-apples f32 comparison against the predicted-
                    // Transform history. A zeroed `t` reads authority as (0,0,0) every
                    // snapshot → perpetual Snap reconcile back to world origin.
                    let world_pos = dequantize_pos(entry.pos_q);
                    snapshots.0.push(SnapshotSample {
                        gid: entry.gid,
                        tick: s.tick,
                        t: world_pos.as_vec3().to_array(), // f32 cell-relative (= absolute in single-cell)
                        r: decode_quat(entry.rot_packed).to_array(),
                        lv: entry.lv,
                        av: entry.av,
                        last_input_seq: entry.last_input_seq,
                        pos: world_pos.to_array(), // f64 absolute world position
                        cell: [0; 3], // single-cell space config
                    });
                }
            }
            SyncEnvelope::Spawn(spawn) => {
                pending_spawns.0.push(ReplicatedSpawn {
                    gid: spawn.gid,
                    entry_id: spawn.entry_id,
                    position: Vec3::from_array(spawn.position),
                });
            }
            SyncEnvelope::Handshake(h) => {
                if !role.is_host() {
                    local.0 = SessionId(h.session);
                    tick.0 = h.tick;
                    info!("[net] handshake accepted: assigned session={}", h.session);
                }
            }
            SyncEnvelope::Ownership(o) => {
                // Clients adopt the host's authoritative who-owns-what table.
                if !role.is_host() {
                    registry.replace_all(o.entries.into_iter().map(|(g, s)| (g, SessionId(s))));
                }
            }
            SyncEnvelope::Profiles(p) => {
                if !role.is_host() {
                    profiles.profiles.clear();
                    profiles.colors.clear();
                    for (session_id, name, color) in p.entries {
                        profiles.profiles.insert(session_id, name);
                        profiles.colors.insert(session_id, color);
                    }
                }
            }
            SyncEnvelope::Ack(_) => { /* MVP is optimistic; acks unused */ }
            SyncEnvelope::Cursor(c) => {
                let session_id = if role.is_host() { sender } else { SessionId(c.session) };
                
                // If it is a client and it's our own cursor, ignore it to prevent echos
                if !role.is_host() && session_id == local.0 {
                    continue;
                }

                // Update presence info in `Presence` resource
                let uid = UserId(session_id.0);
                if let Some(info) = presence.users.get_mut(&uid) {
                    info.cursor = c.cursor;
                    if let Some(color) = c.color {
                        info.color = color;
                    }
                }

                // If we are host, we also need to broadcast this cursor update to all other clients.
                // Relay a clear (cursor None) reliably so peers never keep a ghost cursor;
                // live positions stay on the lossy ControlStream.
                if role.is_host() {
                    if let Some(color) = c.color {
                        profiles.colors.insert(session_id.0, color);
                    }
                    let channel = if c.cursor.is_none() {
                        SyncChannel::CommandBus
                    } else {
                        SyncChannel::ControlStream
                    };
                    outbox.0.push((
                        channel,
                        SyncEnvelope::Cursor(CursorUpdateMsg {
                            session: session_id.0,
                            cursor: c.cursor,
                            color: c.color,
                        }),
                    ));
                }
            }
            SyncEnvelope::TutorStatus(msg) => {
                if role.is_host() {
                    outbox.0.push((
                        SyncChannel::ControlStream,
                        SyncEnvelope::TutorStatus(msg.clone()),
                    ));
                }
                
                // Allow anyone (including host) to follow the teacher, except the teacher themselves
                if msg.tutor_session != local.0 .0 {
                    tutor_status.active_doc = msg.active_doc;
                    tutor_status.active_perspective = msg.active_perspective.clone();
                    tutor_status.target_client = msg.target_client;
                    tutor_status.observe_mode = msg.observe_mode;
                    tutor_status.avatar_state = msg.avatar_state.map(|(pos, rot, cell, ephem_id)| {
                        (
                            Vec3::from_array(pos),
                            Quat::from_array(rot),
                            CellCoord {
                                x: cell[0],
                                y: cell[1],
                                z: cell[2],
                            },
                            ephem_id,
                        )
                    });

                    // Update timestamp and active status
                    let elapsed = time.elapsed_secs_f64();
                    tutor_status.last_received_time = Some(elapsed);

                    let is_targeted = msg.target_client.is_none()
                        || msg.target_client == Some(local.0.0);

                    // If tutor was previously inactive and client is targeted, initialize follow mode
                    if !tutor_status.tutor_active {
                        if is_targeted {
                            tutorial_settings.follow_mode = !msg.allow_free_movement;
                        }
                    } else {
                        // If tutor settings changed from locked to free movement, release follow mode
                        let was_locked = !tutor_status.allow_free_movement;
                        if was_locked && msg.allow_free_movement {
                            tutorial_settings.follow_mode = false;
                        }
                        // If tutor settings changed from free movement to locked, force follow mode
                        if !msg.allow_free_movement {
                            if is_targeted {
                                tutorial_settings.follow_mode = true;
                            } else {
                                tutorial_settings.follow_mode = false;
                            }
                        }
                    }
                    tutor_status.allow_free_movement = msg.allow_free_movement;
                    tutor_status.tutor_active = true;
                }
            }
            SyncEnvelope::StudentStatus(msg) => {
                // Relay so a *client*-tutor receives it too (the host may not be the
                // observer). Each peer's arm below filters by teach/observe/target, so a
                // broadcast is safe — only the observing tutor consumes it. Without this,
                // observe-mode silently works only when the tutor happens to be the host.
                if role.is_host() {
                    outbox.0.push((
                        SyncChannel::ControlStream,
                        SyncEnvelope::StudentStatus(msg.clone()),
                    ));
                }

                // If we are currently observing this student:
                if tutorial_settings.teach_mode
                    && tutorial_settings.target_client == Some(msg.student_session) 
                    && tutorial_settings.observe_mode 
                {
                    tutor_status.observed_student_doc = msg.active_doc;
                    tutor_status.observed_student_perspective = msg.active_perspective.clone();
                    tutor_status.observed_student_avatar_state = msg.avatar_state.map(|(pos, rot, cell, ephem_id)| {
                        (
                            Vec3::from_array(pos),
                            Quat::from_array(rot),
                            CellCoord {
                                x: cell[0],
                                y: cell[1],
                                z: cell[2],
                            },
                            ephem_id,
                        )
                    });
                }
            }
            SyncEnvelope::SharePerspective(msg) => {
                if role.is_host() {
                    // Relay to other clients
                    outbox.0.push((
                        SyncChannel::CommandBus, // reliable
                        SyncEnvelope::SharePerspective(msg.clone()),
                    ));
                }
                
                if msg.tutor_session != local.0 .0 {
                    tutor_status.one_shot_snap_request = Some(msg.clone());
                }
            }
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
        let rot = rotation.map(|r| r.0.as_quat()).unwrap_or(tf.rotation);
        // Absolute world position: prefer the precise avian f64 `Position`; fall
        // back to the f32 `Transform` (as f64) for bodies without a physics Position.
        let pos = position.map(|p| p.0).unwrap_or_else(|| {
            DVec3::new(
                tf.translation.x as f64,
                tf.translation.y as f64,
                tf.translation.z as f64,
            )
        });
        let pos_q = quantize_pos(pos);
        let rot_packed = encode_quat(rot);
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

/// Sync `Presence` users with `SessionProfiles`.
pub fn sync_presence_with_profiles(
    profiles: Res<SessionProfiles>,
    mut presence: ResMut<Presence>,
) {
    if !profiles.is_changed() {
        return;
    }
    // Remove users that are no longer in profiles
    presence.users.retain(|uid, _| profiles.profiles.contains_key(&uid.0));

    // Add/update users from profiles
    for (&session_id, name) in &profiles.profiles {
        let uid = UserId(session_id);
        let color = profiles.colors.get(&session_id).copied().unwrap_or_else(|| generate_user_color(session_id));
        if let Some(info) = presence.users.get_mut(&uid) {
            if info.display_name != *name {
                info.display_name = name.clone();
            }
            info.color = color;
        } else {
            presence.users.insert(uid, PresenceInfo {
                display_name: name.clone(),
                color,
                active_doc: None,
                cursor: None,
            });
        }
    }
}

/// Generate a stable, pleasant color for a user session based on the Catppuccin palette.
pub fn generate_user_color(session_id: u64) -> [u8; 3] {
    let colors = [
        [203, 166, 247], // mauve
        [243, 139, 168], // red
        [250, 179, 135], // peach
        [249, 226, 175], // yellow
        [166, 227, 161], // green
        [148, 226, 213], // teal
        [137, 220, 235], // sky
        [116, 199, 236], // sapphire
        [137, 180, 250], // blue
        [180, 190, 254], // lavender
    ];
    let idx = (session_id as usize) % colors.len();
    colors[idx]
}

/// Seed the local user's cursor/name-tag color from their session id the first frame
/// a session exists, so peers are visually distinct by default instead of all sharing
/// the hardcoded `CursorSettings` default (which, once transmitted, used to overwrite
/// every peer's `generate_user_color` and collapse all tags to one color). A color the
/// user explicitly saved (anything other than the shared default) is left untouched.
/// Runs in `Update` rather than `Startup` so it lands after persisted settings load.
fn seed_local_cursor_color(
    local: Option<Res<LocalSession>>,
    mut settings: ResMut<CursorSettings>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Some(local) = local else {
        return;
    };
    *done = true;
    if settings.color == CursorSettings::default().color {
        settings.color = generate_user_color(local.0 .0);
    }
}

/// Send local mouse cursor updates to the server based on the CursorSettings.
pub fn send_local_cursor_updates(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    settings: Res<CursorSettings>,
    tutorial_settings: Res<TutorialSettings>,
    q_window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut last_sent: Local<Option<[f32; 2]>>,
    mut timer: Local<f32>,
    time: Res<Time>,
    mut outbox: ResMut<SyncOutbox>,
    mut presence: ResMut<Presence>,
) {
    // Only active if we are in a networked session
    if !role.is_networked() {
        return;
    }

    let is_demo = std::env::var("LUNCO_DEMO_CURSOR").is_ok();
    let is_tutor = tutorial_settings.teach_mode;
    if !settings.enabled && !is_demo && !is_tutor {
        // If we previously sent a cursor position, clear it on the network. Send the
        // clear over the *reliable* CommandBus (not the lossy ControlStream the live
        // positions use): a dropped clear would leave a ghost cursor frozen on peers.
        if last_sent.is_some() {
            *last_sent = None;
            outbox.0.push((
                SyncChannel::CommandBus,
                SyncEnvelope::Cursor(CursorUpdateMsg {
                    session: local.0 .0,
                    cursor: None,
                    color: Some(settings.color),
                }),
            ));
            if role.is_host() {
                let uid = UserId(local.0 .0);
                if let Some(info) = presence.users.get_mut(&uid) {
                    info.cursor = None;
                }
            }
        }
        return;
    }

    *timer += time.delta_secs();
    let interval = 1.0 / settings.update_hz.max(0.1);
    if *timer < interval {
        return;
    }

    let current_pos = if is_demo {
        let elapsed = time.elapsed_secs();
        let (w, h) = q_window.iter().next().map(|win| (win.width(), win.height())).unwrap_or((1920.0, 1080.0));
        Some([
            w * (0.5 + (elapsed * 2.0).cos() * 0.25),
            h * (0.5 + (elapsed * 2.0).sin() * 0.25),
        ])
    } else {
        q_window.iter().next().and_then(|window| {
            window.cursor_position().map(|pos| {
                [pos.x, pos.y]
            })
        })
    };

    // Check if we need to send the update
    let should_send = if settings.update_on_delta_only {
        match (current_pos, *last_sent) {
            (Some(curr), Some(last)) => {
                let dx = curr[0] - last[0];
                let dy = curr[1] - last[1];
                // Check if moved more than 2 pixels
                (dx * dx + dy * dy).sqrt() > 2.0
            }
            (None, None) => false,
            _ => true, // transition between Some and None
        }
    } else {
        true
    };

    if should_send {
        *timer = 0.0;
        *last_sent = current_pos;

        // Push update to the outbox so it is sent to the server (or broadcast if host)
        outbox.0.push((
            SyncChannel::ControlStream, // Unreliable fast datagram channel
            SyncEnvelope::Cursor(CursorUpdateMsg {
                session: local.0 .0,
                cursor: current_pos,
                color: Some(settings.color),
            }),
        ));

        // If we are host, we also update our own cursor in our local Presence registry
        if role.is_host() {
            let uid = UserId(local.0 .0);
            if let Some(info) = presence.users.get_mut(&uid) {
                info.cursor = current_pos;
            }
        }
    }
}

/// Send tutor status updates when teach_mode is enabled.
pub fn send_tutor_status_updates(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    settings: Res<TutorialSettings>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<LocalAvatar>>,
    q_reference_frames: Query<&CelestialReferenceFrame>,
    mut timer: Local<f32>,
    time: Res<Time>,
    mut outbox: ResMut<SyncOutbox>,
    #[cfg(feature = "workbench")]
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
) {
    if !role.is_networked() {
        return;
    }

    if !settings.teach_mode {
        return;
    }

    *timer += time.delta_secs();
    if *timer < 0.1 {
        return;
    }
    *timer = 0.0;

    let active_doc = workspace.active_document;
    let active_perspective = {
        #[cfg(feature = "workbench")]
        {
            layout.as_ref().and_then(|l| l.active_perspective()).map(|pid| pid.as_str().to_string())
        }
        #[cfg(not(feature = "workbench"))]
        {
            None
        }
    };
    let avatar_state = q_avatar.iter().next().map(|(transform, cell, child_of)| {
        let ephem_id = q_reference_frames.get(child_of.0).ok().map(|rf| rf.ephemeris_id);
        (
            transform.translation.to_array(),
            transform.rotation.to_array(),
            [cell.x, cell.y, cell.z],
            ephem_id,
        )
    });

    outbox.0.push((
        SyncChannel::ControlStream,
        SyncEnvelope::TutorStatus(TutorStatusMsg {
            tutor_session: local.0 .0,
            active_doc,
            active_perspective,
            avatar_state,
            target_client: settings.target_client,
            observe_mode: settings.observe_mode,
            allow_free_movement: settings.allow_free_movement,
        }),
    ));
}

/// Send student status updates back to the tutor if we are the target and the tutor is observing us.
pub fn send_student_status_updates(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    tutor_status: Res<TutorStatusResource>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<LocalAvatar>>,
    q_reference_frames: Query<&CelestialReferenceFrame>,
    mut timer: Local<f32>,
    time: Res<Time>,
    mut outbox: ResMut<SyncOutbox>,
    #[cfg(feature = "workbench")]
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
) {
    if !role.is_networked() {
        return;
    }

    // Only send if we are the target student and tutor is observing us
    if tutor_status.target_client != Some(local.0 .0) || !tutor_status.observe_mode {
        return;
    }

    *timer += time.delta_secs();
    if *timer < 0.1 {
        return;
    }
    *timer = 0.0;

    let active_doc = workspace.active_document;
    let active_perspective = {
        #[cfg(feature = "workbench")]
        {
            layout.as_ref().and_then(|l| l.active_perspective()).map(|pid| pid.as_str().to_string())
        }
        #[cfg(not(feature = "workbench"))]
        {
            None
        }
    };
    let avatar_state = q_avatar.iter().next().map(|(transform, cell, child_of)| {
        let ephem_id = q_reference_frames.get(child_of.0).ok().map(|rf| rf.ephemeris_id);
        (
            transform.translation.to_array(),
            transform.rotation.to_array(),
            [cell.x, cell.y, cell.z],
            ephem_id,
        )
    });

    outbox.0.push((
        SyncChannel::ControlStream,
        SyncEnvelope::StudentStatus(StudentStatusMsg {
            student_session: local.0 .0,
            active_doc,
            active_perspective,
            avatar_state,
        }),
    ));
}

/// Snap every `LocalAvatar` to a target pose, migrating to the grid that owns
/// `grid_ephemeris_id` (resolved via `CelestialReferenceFrame`) when it differs
/// from the avatar's current parent. Shared by all three mirroring paths
/// (follow, observe, one-shot look-at) so the snap logic lives in one place.
fn snap_avatars_to(
    commands: &mut Commands,
    q_avatar: &mut Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<LocalAvatar>,
    >,
    q_reference_frames: &Query<(Entity, &CelestialReferenceFrame)>,
    pos: Vec3,
    rot: Quat,
    target_cell: CellCoord,
    grid_ephemeris_id: Option<i32>,
) {
    let target_grid_entity = grid_ephemeris_id.and_then(|ephem_id| {
        q_reference_frames
            .iter()
            .find(|(_, rf)| rf.ephemeris_id == ephem_id)
            .map(|(ent, _)| ent)
    });
    // `freeflight_system` rebuilds `Transform.rotation` from the camera's stored
    // `yaw`/`pitch` every frame (`Quat::from_euler(YXZ, yaw, pitch, 0)`), so
    // writing `transform.rotation` alone is clobbered next frame. Decompose the
    // target rotation back into yaw/pitch and write those so the camera system
    // reproduces the mirrored orientation instead of fighting it.
    let (target_yaw, target_pitch, _roll) = rot.to_euler(EulerRot::YXZ);
    let target_tf = Transform::from_translation(pos).with_rotation(rot);
    for (avatar_entity, mut transform, mut cell_coord, child_of, freeflight) in q_avatar.iter_mut() {
        if let Some(mut ff) = freeflight {
            if ff.yaw != target_yaw {
                ff.yaw = target_yaw;
            }
            if ff.pitch != target_pitch {
                ff.pitch = target_pitch;
            }
        }
        let target_grid = target_grid_entity.unwrap_or(child_of.0);
        if child_of.0 != target_grid {
            lunco_core::attach::migrate_to_grid(
                commands,
                avatar_entity,
                target_grid,
                target_cell,
                target_tf,
            );
        } else {
            if transform.translation != target_tf.translation {
                transform.translation = target_tf.translation;
            }
            if transform.rotation != target_tf.rotation {
                transform.rotation = target_tf.rotation;
            }
            if *cell_coord != target_cell {
                *cell_coord = target_cell;
            }
        }
    }
}

/// Should local input be suppressed this frame because we are mirroring someone
/// else's perspective? True when we are a follower locked to the tutor, or when
/// we are the tutor observing a student (so our own avatar doesn't fight the
/// mirror). The observed student itself is never blocked — they move, we watch.
fn perspective_inputs_blocked(
    settings: &TutorialSettings,
    tutor_status: &TutorStatusResource,
    local: Option<&LocalSession>,
) -> bool {
    // Tutor observing a target student: freeze the tutor's own avatar control.
    if settings.teach_mode && settings.observe_mode && settings.target_client.is_some() {
        return true;
    }
    if !settings.follow_mode {
        return false;
    }
    let is_targeted = tutor_status.target_client.is_none()
        || local.map_or(false, |loc| tutor_status.target_client == Some(loc.0 .0));
    if !is_targeted {
        return false;
    }
    // If we are the observed student, don't block — we move freely, tutor watches.
    if let Some(loc) = local {
        if tutor_status.target_client == Some(loc.0 .0) && tutor_status.observe_mode {
            return false;
        }
    }
    true
}

/// Mirror the tutor's status on clients when follow_mode is enabled.
pub fn apply_tutorial_mirroring(
    mut commands: Commands,
    settings: Res<TutorialSettings>,
    mut tutor_status: ResMut<TutorStatusResource>,
    local: Option<Res<LocalSession>>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<LocalAvatar>,
    >,
    q_reference_frames: Query<(Entity, &CelestialReferenceFrame)>,
    #[cfg(feature = "workbench")]
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
) {
    // Case 1: Student is in follow mode, mirroring tutor status
    if settings.follow_mode {
        // If we are being observed, don't mirror (since the tutor mirrors us, mirroring back creates a loop)
        if let Some(loc) = &local {
            if tutor_status.target_client == Some(loc.0.0) && tutor_status.observe_mode {
                return;
            }
        }

        // Check if tutor status is targeted at us (None = everyone)
        let is_targeted = tutor_status.target_client.is_none() 
            || local.as_ref().map_or(false, |loc| tutor_status.target_client == Some(loc.0.0));

        if is_targeted {
            // Mirror active document
            if workspace.active_document != tutor_status.active_doc {
                workspace.active_document = tutor_status.active_doc;
            }

            // Mirror active perspective
            #[cfg(feature = "workbench")]
            if let Some(ref target_persp) = tutor_status.active_perspective {
                if let Some(ref l) = layout {
                    let current_persp = l.active_perspective().map(|pid| pid.as_str());
                    if current_persp != Some(target_persp) {
                        commands.trigger(lunco_workbench::perspective_command::ActivatePerspective { id: target_persp.clone() });
                    }
                }
            }

            // Mirror avatar transform, cell coordinate, and grid parent
            if let Some((pos, rot, cell, grid_ephemeris_id)) = tutor_status.avatar_state {
                snap_avatars_to(
                    &mut commands,
                    &mut q_avatar,
                    &q_reference_frames,
                    pos,
                    rot,
                    cell,
                    grid_ephemeris_id,
                );
            }
        }
    }

    // Case 2: Tutor is in teach mode and observe mode is enabled, mirroring target student
    if settings.teach_mode && settings.observe_mode {
        if settings.target_client.is_some() {
            // Mirror active document from the observed student
            if workspace.active_document != tutor_status.observed_student_doc {
                workspace.active_document = tutor_status.observed_student_doc;
            }

            // Mirror active perspective from the observed student
            #[cfg(feature = "workbench")]
            if let Some(ref target_persp) = tutor_status.observed_student_perspective {
                if let Some(ref l) = layout {
                    let current_persp = l.active_perspective().map(|pid| pid.as_str());
                    if current_persp != Some(target_persp) {
                        commands.trigger(lunco_workbench::perspective_command::ActivatePerspective { id: target_persp.clone() });
                    }
                }
            }

            // Mirror avatar transform, cell coordinate, and grid parent from the observed student
            if let Some((pos, rot, cell, grid_ephemeris_id)) = tutor_status.observed_student_avatar_state {
                snap_avatars_to(
                    &mut commands,
                    &mut q_avatar,
                    &q_reference_frames,
                    pos,
                    rot,
                    cell,
                    grid_ephemeris_id,
                );
            }
        }
    }

    // Case 3: One-shot perspective snap request (Look-At)
    if let Some(msg) = tutor_status.one_shot_snap_request.take() {
        // Snap active document
        if workspace.active_document != msg.active_doc {
            workspace.active_document = msg.active_doc;
        }

        // Snap active perspective
        #[cfg(feature = "workbench")]
        if let Some(ref target_persp) = msg.active_perspective {
            if let Some(ref l) = layout {
                let current_persp = l.active_perspective().map(|pid| pid.as_str());
                if current_persp != Some(target_persp) {
                    commands.trigger(lunco_workbench::perspective_command::ActivatePerspective { id: target_persp.clone() });
                }
            }
        }

        // Snap avatar transform, cell, and grid parent
        if let Some((pos, rot, cell, grid_ephemeris_id)) = msg.avatar_state {
            snap_avatars_to(
                &mut commands,
                &mut q_avatar,
                &q_reference_frames,
                Vec3::from_array(pos),
                Quat::from_array(rot),
                CellCoord { x: cell[0], y: cell[1], z: cell[2] },
                grid_ephemeris_id,
            );
        }
        info!("[net] Snapped to tutor's perspective (one-shot).");
    }
}

/// Updates local tutor active/inactive lifecycle status.
pub fn update_tutor_lifecycle(
    time: Res<Time>,
    mut tutor_status: ResMut<TutorStatusResource>,
    mut settings: ResMut<TutorialSettings>,
) {
    if let Some(last_time) = tutor_status.last_received_time {
        let elapsed = time.elapsed_secs_f64();
        if elapsed - last_time > 1.0 {
            // Tutor timed out!
            if tutor_status.tutor_active {
                tutor_status.tutor_active = false;
                tutor_status.active_doc = None;
                tutor_status.avatar_state = None;
                settings.follow_mode = false;
                info!("[net] Tutor inactive, exiting follow mode.");
            }
        }
    }
}

/// Block Bevy key/mouse button input resources in follow mode.
pub fn block_bevy_inputs(
    settings: Res<TutorialSettings>,
    tutor_status: Res<TutorStatusResource>,
    local: Option<Res<LocalSession>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse_buttons: ResMut<ButtonInput<MouseButton>>,
) {
    if perspective_inputs_blocked(&settings, &tutor_status, local.as_deref()) {
        keys.clear();
        mouse_buttons.clear();
    }
}

/// Reset Leafwing action states in follow mode so no actions fire.
pub fn block_action_states(
    settings: Res<TutorialSettings>,
    tutor_status: Res<TutorStatusResource>,
    local: Option<Res<LocalSession>>,
    mut q_user_intent: Query<&mut ActionState<lunco_core::UserIntent>>,
    mut q_vessel_intent: Query<&mut ActionState<lunco_controller::VesselIntent>>,
) {
    if perspective_inputs_blocked(&settings, &tutor_status, local.as_deref()) {
        for mut state in &mut q_user_intent {
            *state = ActionState::default();
        }
        for mut state in &mut q_vessel_intent {
            *state = ActionState::default();
        }
    }
}

// ── Plugin (always-on; inert in single-player) ────────────────────────────────

/// Registers the wire substrate. Added unconditionally by `LunCoApiPlugin`; all
/// its systems early-return under [`NetworkRole::Standalone`], so single-player
/// pays nothing.
pub struct SyncPlugin;

/// Startup system to register the host session in SessionRbac (Owner role, authenticated).
fn setup_host_rbac(
    local: Res<LocalSession>,
    mut rbac: ResMut<lunco_core::session::SessionRbac>,
) {
    rbac.sessions.insert(local.0.0, lunco_core::session::UserSession {
        session_id: local.0,
        username: "Host".to_string(),
        role: lunco_core::session::AuthorityRole::Owner,
        authenticated: true,
        // No token: `is_authorized` does not verify tokens yet. Carrying a fake
        // constant here only made the auth look stronger than it is. Populate this
        // (and check it) when real authentication lands.
        token: None,
    });
}

/// Observer system to handle Profile updates: marks client as authenticated and promotes role.
fn on_update_profile_rbac(
    trigger: On<lunco_avatar::UpdateProfile>,
    guard: Res<lunco_core::SyncApplyGuard>,
    local: Res<LocalSession>,
    mut rbac: ResMut<lunco_core::session::SessionRbac>,
) {
    let origin = guard.0.unwrap_or(local.0);
    let username = trigger.event().name.clone();

    let session = rbac.sessions.entry(origin.0).or_insert_with(|| {
        lunco_core::session::UserSession {
            session_id: origin,
            username: username.clone(),
            role: lunco_core::session::AuthorityRole::Operator, // Promote to Operator on name set
            authenticated: true, // Mark authenticated!
            token: None, // Tokens are not verified yet; see `setup_host_rbac`.
        }
    });
    session.username = username;
    session.authenticated = true;
    if session.role == lunco_core::session::AuthorityRole::Observer {
        session.role = lunco_core::session::AuthorityRole::Operator; // Promote observers to operators on name set
    }
    info!("[net] RBAC: session {} authenticated as '{}' with role {:?}", origin.0, session.username, session.role);
}

/// Set Teach Mode command.
#[lunco_core::Command(default)]
pub struct SetTeachMode {
    pub enabled: bool,
}

/// Set Follow Mode command.
#[lunco_core::Command(default)]
pub struct SetFollowMode {
    pub enabled: bool,
}

/// Set Target Client command.
#[lunco_core::Command(default)]
pub struct SetTargetClient {
    pub target: Option<u64>,
}

/// Set Observe Mode command.
#[lunco_core::Command(default)]
pub struct SetObserveMode {
    pub enabled: bool,
}

#[lunco_core::on_command(SetTeachMode)]
fn on_set_teach_mode(
    trigger: On<SetTeachMode>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.teach_mode = trigger.event().enabled;
    if !settings.teach_mode {
        settings.observe_mode = false;
    }
    info!("[net] Command: Teach Mode set to {}", settings.teach_mode);
}

#[lunco_core::on_command(SetFollowMode)]
fn on_set_follow_mode(
    trigger: On<SetFollowMode>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.follow_mode = trigger.event().enabled;
    info!("[net] Command: Follow Mode set to {}", settings.follow_mode);
}

#[lunco_core::on_command(SetTargetClient)]
fn on_set_target_client(
    trigger: On<SetTargetClient>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.target_client = trigger.event().target;
    info!("[net] Command: Target Client set to {:?}", settings.target_client);
}

#[lunco_core::on_command(SetObserveMode)]
fn on_set_observe_mode(
    trigger: On<SetObserveMode>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.observe_mode = trigger.event().enabled;
    info!("[net] Command: Observe Mode set to {}", settings.observe_mode);
}

/// Set Allow Free Movement command.
#[lunco_core::Command(default)]
pub struct SetAllowFreeMovement {
    pub enabled: bool,
}

#[lunco_core::on_command(SetAllowFreeMovement)]
fn on_set_allow_free_movement(
    trigger: On<SetAllowFreeMovement>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.allow_free_movement = trigger.event().enabled;
    info!("[net] Command: Allow Free Movement set to {}", settings.allow_free_movement);
}

/// Share Perspective command (Look-At).
#[lunco_core::Command(default)]
pub struct SharePerspective {}

#[lunco_core::on_command(SharePerspective)]
fn on_share_perspective(
    _trigger: On<SharePerspective>,
    local: Res<LocalSession>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    q_avatar: Query<(&Transform, &CellCoord, &ChildOf), With<LocalAvatar>>,
    q_reference_frames: Query<&CelestialReferenceFrame>,
    mut outbox: ResMut<SyncOutbox>,
    #[cfg(feature = "workbench")]
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
) {
    let active_doc = workspace.active_document;
    let active_perspective = {
        #[cfg(feature = "workbench")]
        {
            layout.as_ref().and_then(|l| l.active_perspective()).map(|pid| pid.as_str().to_string())
        }
        #[cfg(not(feature = "workbench"))]
        {
            None
        }
    };
    let avatar_state = q_avatar.iter().next().map(|(transform, cell, child_of)| {
        let ephem_id = q_reference_frames.get(child_of.0).ok().map(|rf| rf.ephemeris_id);
        (
            transform.translation.to_array(),
            transform.rotation.to_array(),
            [cell.x, cell.y, cell.z],
            ephem_id,
        )
    });

    outbox.0.push((
        SyncChannel::CommandBus, // reliable
        SyncEnvelope::SharePerspective(SharePerspectiveMsg {
            tutor_session: local.0 .0,
            active_doc,
            active_perspective,
            avatar_state,
        }),
    ));
    info!("[net] Shared perspective command sent.");
}

lunco_core::register_commands!(
    on_set_teach_mode,
    on_set_follow_mode,
    on_set_target_client,
    on_set_observe_mode,
    on_share_perspective,
    on_set_allow_free_movement,
);

/// Host: when ObstacleFieldSpec is updated, broadcast UpdateObstacleFieldSpec command to all clients.
pub fn sync_obstacle_field_spec(
    role: Res<NetworkRole>,
    spec: Option<Res<lunco_obstacle_field::ObstacleFieldSpec>>,
    mut outbox: ResMut<SyncOutbox>,
    type_registry: Res<AppTypeRegistry>,
    local: Res<LocalSession>,
) {
    if !role.is_host() {
        return;
    }
    let Some(spec) = spec else {
        return;
    };
    if spec.is_changed() {
        let cmd = lunco_obstacle_field::plugin::UpdateObstacleFieldSpec {
            spec: spec.clone(),
        };
        let type_name = "UpdateObstacleFieldSpec".to_string();
        let type_reg = type_registry.read();
        if type_reg.get_with_short_type_path(&type_name).is_some() {
            let serializer = TypedReflectSerializer::new(&cmd, &type_reg);
            if let Ok(data_val) = serde_json::to_value(&serializer) {
                if let Ok(data) = serde_json::to_string(&data_val) {
                    let mut mutation = Mutation::local(SyncCommand { type_name, data });
                    mutation.origin = local.0;
                    outbox.0.push((SyncChannel::CommandBus, SyncEnvelope::Command(mutation)));
                    info!("[net] Broadcast ObstacleFieldSpec update to clients.");
                }
            }
        }
    }
}

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
            .init_resource::<CursorSettings>()
            .init_resource::<TutorialSettings>()
            .init_resource::<TutorStatusResource>()
            .init_resource::<Presence>()
            .register_settings_section::<CursorSettings>()
            .register_settings_section::<TutorialSettings>()
            .init_resource::<SyncChannelRegistry>()
            .add_observer(apply_sync_command)
            .add_observer(on_update_profile_rbac)
            .add_systems(Startup, setup_host_rbac)
            .add_systems(PreUpdate, block_bevy_inputs)
            .add_systems(Update, (
                drain_sync_inbox,
                broadcast_new_spawns,
                sync_presence_with_profiles,
                seed_local_cursor_color,
                send_local_cursor_updates,
                send_tutor_status_updates,
                send_student_status_updates,
                apply_tutorial_mirroring,
                update_tutor_lifecycle,
                block_action_states,
                sync_obstacle_field_spec,
            ))
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
            // pose and its `last_input_seq` ack are a consistent post-step pair  —
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
        register_all_commands(app);
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
