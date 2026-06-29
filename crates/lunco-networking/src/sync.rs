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

/// Host → clients: the networked entity with this id was removed on the host;
/// despawn its local proxy. The inverse of [`SpawnReplicationMsg`] — without it a
/// host-side removal (Inspector delete, cosim teardown) leaves a frozen kinematic
/// ghost proxy pinned at its last replicated pose on every client, forever.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DespawnReplicationMsg {
    pub gid: u64,
}

/// Host → a freshly-connected client: your **server-assigned** session id, the
/// current tick, and a server-issued auth token. The session id is allocated from
/// server entropy (the client cannot pick or guess it — review H4/H5); the token
/// is the credential the client holds to prove it owns this session (review M2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandshakeMsg {
    pub session: u64,
    pub tick: u64,
    pub token: String,
}

/// Client-side store for the server-issued auth token received in the handshake.
/// Empty on host/standalone. Held as proof-of-possession of the server-assigned
/// session for a future challenge/elevation path (review M2).
#[derive(Resource, Default, Clone, Debug)]
pub struct SessionCredential {
    pub token: Option<String>,
}

/// Two low-use resources bundled so [`drain_sync_inbox`] stays within Bevy's
/// 16-param ceiling: the client's session-credential store (handshake) and the
/// clock (tutor-status timestamps).
#[derive(bevy::ecs::system::SystemParam)]
pub struct InboundClientCtx<'w> {
    credential: ResMut<'w, SessionCredential>,
    time: Res<'w, Time>,
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

/// Avatar pose as it crosses the wire: position, rotation, big-space cell, and
/// optional ephemeris id.
pub type WireAvatarState = ([f32; 3], [f32; 4], [i64; 3], Option<i32>);
/// Decoded avatar pose (as held in [`TutorStatusResource`]).
pub type AvatarState = (Vec3, Quat, CellCoord, Option<i32>);

/// Pack an avatar pose for the wire. One definition shared by the tutor, student,
/// and share-perspective senders so the field order can't drift between them.
fn encode_avatar_state(transform: &Transform, cell: &CellCoord, ephem_id: Option<i32>) -> WireAvatarState {
    (
        transform.translation.to_array(),
        transform.rotation.to_array(),
        [cell.x, cell.y, cell.z],
        ephem_id,
    )
}

/// Inverse of [`encode_avatar_state`]: decode a wire avatar pose for local use.
fn decode_avatar_state(w: WireAvatarState) -> AvatarState {
    let (pos, rot, cell, ephem_id) = w;
    (
        Vec3::from_array(pos),
        Quat::from_array(rot),
        CellCoord { x: cell[0], y: cell[1], z: cell[2] },
        ephem_id,
    )
}

/// Build the `(session, name, color)` wire rows for a [`ProfilesMsg`] from the
/// authoritative [`SessionProfiles`], filling a deterministic per-session color
/// for any session that has no explicit one. Shared by the periodic broadcast and
/// the connect-time replay so the two can't drift.
pub(crate) fn profile_wire_entries(profiles: &SessionProfiles) -> Vec<(u64, String, [u8; 3])> {
    profiles
        .profiles
        .iter()
        .map(|(&s, n)| {
            let color = profiles.colors.get(&s).copied().unwrap_or_else(|| generate_user_color(s));
            (s, n.clone(), color)
        })
        .collect()
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
    /// Local consent: whether *this* user allows a **broadcasting** tutor (one that
    /// targets Everyone) to lock its view. Opt-in, default `false` — a broadcast
    /// `TutorStatus` can no longer freeze a peer that hasn't agreed to follow. An
    /// explicitly-targeted student (`target_client == me`) is a deliberate 1:1
    /// selection and is honored regardless of this flag. Session-transient (`skip`).
    #[serde(skip)]
    pub follow_opt_in: bool,
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
            // Opt-out by default: a broadcast tutor must not seize a peer that
            // hasn't consented. See `follow_opt_in`.
            follow_opt_in: false,
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
    // Appended LAST on purpose: the bincode codec is positional, so inserting a
    // variant mid-enum shifts every later discriminant and breaks a version-skewed
    // peer (stale wasm bundle vs fresh host). Adding at the end leaves all existing
    // variants' discriminants unchanged; only peers that know `Despawn` use it.
    Despawn(DespawnReplicationMsg),
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
    /// One replay window **per origin**. A single shared FIFO would let one chatty
    /// peer evict another peer's recent `op_id`s, shrinking each peer's effective
    /// replay protection to ~`cap`/N. A per-origin window gives every peer the full
    /// `cap` regardless of others' traffic. Memory tracks live sessions: the host
    /// drops a window via [`forget`](Self::forget) on disconnect.
    per_origin: HashMap<u64, PeerWindow>,
    cap: usize,
}

/// Bounded FIFO of seen `op_id`s for a single origin.
#[derive(Default)]
struct PeerWindow {
    seen: HashSet<u64>,
    order: VecDeque<u64>,
}

impl Default for SyncDedup {
    fn default() -> Self {
        Self {
            per_origin: HashMap::new(),
            cap: 8192,
        }
    }
}

impl SyncDedup {
    /// `true` if `(origin, op)` is new (apply it); `false` if already seen
    /// (drop). The same `op` from two different origins is **not** a duplicate.
    pub fn check_and_insert(&mut self, origin: SessionId, op: OpId) -> bool {
        let cap = self.cap;
        let w = self.per_origin.entry(origin.0).or_default();
        if !w.seen.insert(op.0) {
            return false;
        }
        w.order.push_back(op.0);
        if w.order.len() > cap {
            if let Some(old) = w.order.pop_front() {
                w.seen.remove(&old);
            }
        }
        true
    }

    /// Drop a disconnected origin's window so tracked memory follows live sessions
    /// (called by the host on client disconnect). A reconnecting session gets a
    /// fresh window; its `op_id`s restart from the new process's entropy anyway.
    pub fn forget(&mut self, origin: SessionId) {
        self.per_origin.remove(&origin.0);
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

    // One registry read guard across both reflect passes (serialize + id-globalize)
    // — no need to drop and re-acquire the lock between them.
    let type_reg = type_registry.read();

    // Serialize through the SAME reflect path the apply side deserializes with.
    let mut data = {
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
    globalize_command_ids(
        &mut data,
        std::any::TypeId::of::<C>(),
        &type_reg,
        &entity_registry,
    );
    drop(type_reg);

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
        // and possession skips local camera-bind for a remote origin. Set/clear is
        // balanced without RAII: every early `return` above happens *before* the set,
        // and the only step between set and clear is `trigger` itself — whose sole
        // panic source (`FromReflect`) is pre-checked above, and any panic in Bevy's
        // command queue aborts the process anyway, so a stuck guard can't outlive it.
        world.resource_mut::<SyncApplyGuard>().0 = Some(origin);
        reflect_event.trigger(world, reflected.as_ref(), &type_reg);
        world.resource_mut::<SyncApplyGuard>().0 = None;
    });
}

// ── Inbox drain (commands / snapshots / spawns / handshake) ───────────────────

/// Host relay gate for avatar-control envelopes (`TutorStatus`/`StudentStatus`/
/// `SharePerspective`). These seize or redirect targeted peers' avatar camera +
/// input, so the host must drop them from an *unauthenticated* sender (and the
/// caller stamps the sender identity to prevent impersonation). Single chokepoint
/// for that policy — change the required role here, not in each arm.
///
/// Returns whether the arm should keep processing:
/// - non-host peer → `true` (clients never gate; they trust the host-stamped relay),
/// - host + `Operator`-or-higher sender → `true` (caller stamps + relays + consumes),
/// - host + Observer/unauthenticated sender → `false` (caller `continue`s: not relayed, not consumed).
///
/// Gated at **`Operator`**, not `Observer`. Every raw connection is auto-inserted as
/// an authenticated `Observer` (`server.rs`), so an Observer floor is no gate at all:
/// any peer could emit `TutorStatus { allow_free_movement: false }` and freeze every
/// other peer's input + seize their camera. `Operator` is the same bar the control
/// commands use, and a client reaches it only by sending an `UpdateProfile`
/// (`on_update_profile_rbac`) — an intentional act, not a side effect of connecting.
/// A legitimate tutor sets a profile before teaching, so this does not drop real
/// tutors; it drops the passive/auto Observer that the threat relies on.
///
/// This closes the unprivileged-floor hole. It does NOT make teaching fully safe:
/// `Operator` promotion is still token-less self-promotion (see `on_update_profile_rbac`),
/// and a broadcast `TutorStatus` still locks *all* peers rather than only opted-in
/// followers. Real fixes — verified-token auth and per-peer follow opt-in — are
/// tracked separately (review H4/M2 and target opt-in).
#[inline]
fn authed_for_avatar_relay(
    role: &NetworkRole,
    rbac: &lunco_core::session::SessionRbac,
    sender: SessionId,
) -> bool {
    !role.is_host() || rbac.is_authorized(sender, lunco_core::session::AuthorityRole::Operator)
}

#[allow(clippy::too_many_arguments)]
pub fn drain_sync_inbox(
    mut inbox: ResMut<SyncInbox>,
    mut commands: Commands,
    role: Res<NetworkRole>,
    mut local: ResMut<LocalSession>,
    mut ctx: InboundClientCtx,
    mut tick: ResMut<SimTick>,
    mut pending_spawns: ResMut<PendingReplicatedSpawns>,
    mut snapshots: ResMut<IncomingSnapshots>,
    mut registry: ResMut<SessionRegistry>,
    // Host gates tutor/student/perspective relays on the sender's role (same bar
    // as the other state-mutating commands) and binds their claimed session to the
    // actual sender — these messages seize peers' avatar camera + input.
    rbac: Res<lunco_core::session::SessionRbac>,
    mut profiles: ResMut<SessionProfiles>,
    mut presence: ResMut<Presence>,
    mut outbox: ResMut<SyncOutbox>,
    mut tutor_status: ResMut<TutorStatusResource>,
    mut tutorial_settings: ResMut<TutorialSettings>,
    // Resolves a replicated `Despawn`'s gid back to its local proxy entity (O(1)).
    entities: Res<ApiEntityRegistry>,
) {
    if inbox.0.is_empty() {
        return;
    }
    let mut drained: Vec<(SessionId, SyncEnvelope)> = std::mem::take(&mut inbox.0);
    // Order within a frame: possession/structural commands BEFORE control commands.
    // Only sort when a control command is actually present — otherwise every entry
    // keys to 0 (the common case: a batch of snapshots/cursors) and the sort is pure
    // overhead. The stable sort preserves arrival order among equal keys.
    let has_control = drained.iter().any(|(_, env)| {
        matches!(env, SyncEnvelope::Command(m) if is_control_command(&m.payload.type_name))
    });
    if has_control {
        drained.sort_by_key(|(_, env)| match env {
            SyncEnvelope::Command(m) if is_control_command(&m.payload.type_name) => 1u8,
            _ => 0,
        });
    }
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
                // Authority: only the host produces snapshots; clients adopt them.
                // The host must never ingest a client-sent Snapshot into its own
                // `IncomingSnapshots` (it would let a client move host-authoritative
                // bodies). Same guard the Despawn arm enforces.
                if role.is_host() {
                    continue;
                }
                // Adopt host simulation clock (keeps remote interpolation in sync).
                let host_tick = s.tick;
                if tick.0 < host_tick {
                    tick.0 = host_tick;
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
                // Authority: only the host spawns replicated entities; clients apply.
                // Without this guard any client could push an arbitrary catalog spawn
                // pinned to an attacker-chosen gid (`apply_replicated_spawns` is not
                // role-gated), colliding with an existing GlobalEntityId and corrupting
                // registry resolution / snapshot routing / despawn. Mirror Despawn.
                if role.is_host() {
                    continue;
                }
                pending_spawns.0.push(ReplicatedSpawn {
                    gid: spawn.gid,
                    entry_id: spawn.entry_id,
                    position: Vec3::from_array(spawn.position),
                });
            }
            SyncEnvelope::Despawn(d) => {
                // Authority: only the host removes entities; clients apply. The
                // host already despawned locally (that's what emitted this), so it
                // ignores any inbound Despawn (a client can't despawn host state).
                if !role.is_host() {
                    // Cancel a still-queued spawn for this gid: a Spawn followed by a
                    // Despawn that lands before `apply_replicated_spawns` instantiates
                    // the proxy would otherwise be a no-op below and leave a permanent
                    // ghost once the proxy is later created.
                    pending_spawns.0.retain(|s| s.gid != d.gid);
                    // O(1) gid→entity via the canonical registry (the same map the
                    // render path resolves), not a linear scan of every gid'd entity.
                    if let Some(proxy) = entities.resolve(&GlobalEntityId::from_raw(d.gid)) {
                        commands.entity(proxy).despawn();
                    }
                }
            }
            SyncEnvelope::Handshake(h) => {
                if !role.is_host() {
                    local.0 = SessionId(h.session);
                    tick.0 = h.tick;
                    // Hold the server-issued token. The host already binds this
                    // client's authority to its connection (the inbound sender is the
                    // server-assigned session, never a value from the wire), so the
                    // token is the client's proof-of-possession for a future
                    // challenge/elevation path — stored now so the handshake field is
                    // load-bearing rather than dead.
                    ctx.credential.token = Some(h.token);
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
            SyncEnvelope::TutorStatus(mut msg) => {
                // Host authz + anti-spoof: tutor mode seizes targeted peers' avatar
                // camera and locks their input. Gate (one chokepoint) then bind the
                // tutor identity to the actual sender (mirroring the `Cursor` arm) so a
                // peer can't claim to be another session. See [`authed_for_avatar_relay`].
                if !authed_for_avatar_relay(&role, &rbac, sender) {
                    continue;
                }
                if role.is_host() {
                    msg.tutor_session = sender.0;
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
                    tutor_status.avatar_state = msg.avatar_state.map(decode_avatar_state);

                    // Update timestamp and active status
                    let elapsed = ctx.time.elapsed_secs_f64();
                    tutor_status.last_received_time = Some(elapsed);

                    // Per-peer follow opt-in. The network may force *this* peer's
                    // follow lock only when the peer consents:
                    //   - explicit target (`target_client == me`) → deliberate 1:1
                    //     selection, always honored;
                    //   - broadcast (`target_client == None`) → only locks peers that
                    //     opted in (`follow_opt_in`).
                    // A non-consenting peer's `follow_mode` is left untouched (its own
                    // manual choice stands), so one broadcast can no longer freeze every
                    // peer in the session — the residual half of review H2.
                    let is_explicitly_targeted = msg.target_client == Some(local.0.0);
                    let is_broadcast = msg.target_client.is_none();
                    let consented = is_explicitly_targeted
                        || (is_broadcast && tutorial_settings.follow_opt_in);

                    if consented {
                        if !tutor_status.tutor_active {
                            // First status from this tutor: enter follow unless the
                            // tutor allows free movement.
                            tutorial_settings.follow_mode = !msg.allow_free_movement;
                        } else {
                            let was_locked = !tutor_status.allow_free_movement;
                            // Tutor flipped locked → free: release the lock (the peer
                            // may still re-follow voluntarily).
                            if was_locked && msg.allow_free_movement {
                                tutorial_settings.follow_mode = false;
                            }
                            // Tutor flipped free → locked: re-engage the lock.
                            if !msg.allow_free_movement {
                                tutorial_settings.follow_mode = true;
                            }
                        }
                    }
                    tutor_status.allow_free_movement = msg.allow_free_movement;
                    tutor_status.tutor_active = true;
                }
            }
            SyncEnvelope::StudentStatus(mut msg) => {
                // Relay so a *client*-tutor receives it too (the host may not be the
                // observer). Each peer's arm below filters by teach/observe/target, so a
                // broadcast is safe — only the observing tutor consumes it. Without this,
                // observe-mode silently works only when the tutor happens to be the host.
                //
                // Anti-spoof: bind the report to its actual sender so a peer can't
                // impersonate another student and pollute the observing tutor's
                // mirrored view. See [`authed_for_avatar_relay`].
                if !authed_for_avatar_relay(&role, &rbac, sender) {
                    continue;
                }
                if role.is_host() {
                    msg.student_session = sender.0;
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
                    tutor_status.observed_student_avatar_state = msg.avatar_state.map(decode_avatar_state);
                }
            }
            SyncEnvelope::SharePerspective(mut msg) => {
                // Same authz + anti-spoof as TutorStatus: a SharePerspective snaps
                // every targeted peer's avatar/camera to the sender's view. See
                // [`authed_for_avatar_relay`].
                if !authed_for_avatar_relay(&role, &rbac, sender) {
                    continue;
                }
                if role.is_host() {
                    msg.tutor_session = sender.0;
                    // Relay to other clients
                    outbox.0.push((
                        SyncChannel::CommandBus, // reliable
                        SyncEnvelope::SharePerspective(msg.clone()),
                    ));
                }

                if msg.tutor_session != local.0 .0 {
                    tutor_status.one_shot_snap_request = Some(msg);
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
    mut last_sent: Local<HashMap<u64, ([i32; 3], u32, u32)>>,
    // gids currently emitting a non-finite pose; warn once per gid, clear on recovery.
    mut nonfinite_warned: Local<HashSet<u64>>,
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
    let mut live: HashSet<u64> = HashSet::with_capacity(last_sent.len());
    for (gid, tf, lin, ang, position, rotation, _cell) in q.iter() {
        let key = gid.get();
        live.insert(key);
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
        // A cosim blow-up emits a NaN/inf `Position` — or velocity, which integrates
        // to inf a frame EARLIER than the pose does. `quantize_pos` maps NaN→0 via
        // `as i32`, decoding to world origin (→ a perpetual snap-to-origin reconcile
        // per the zeroed-authority warning in `drain_sync_inbox`); a non-finite lv/av
        // poisons the client's dead-reckoning the same way (`pos += lv*dt` → ±inf).
        // Check all four and drop the entry so the proxy latches its last good pose.
        // Warn once per gid (cleared on recovery) to avoid per-tick spam.
        let lv = lin.map(|v| v.0.as_vec3()).unwrap_or(Vec3::ZERO);
        let av = ang.map(|v| v.0.as_vec3()).unwrap_or(Vec3::ZERO);
        if !pos.is_finite() || !rot.is_finite() || !lv.is_finite() || !av.is_finite() {
            if nonfinite_warned.insert(key) {
                warn!("[sync] non-finite pose/velocity for gid {key}, skipping: pos={pos:?} rot={rot:?} lv={lv:?} av={av:?}");
            }
            continue;
        }
        nonfinite_warned.remove(&key);
        let pos_q = quantize_pos(pos);
        let rot_packed = encode_quat(rot);
        // Fold the reconcile ack into the change key. An owned body that is
        // quantize-stationary (pushing an obstacle, brake+throttle, steering in
        // place) keeps advancing its applied input seq host-side, but the client's
        // `reconcile_owned_prediction` only evaluates a correction on a *new* ack.
        // Keying on `last_input_seq` too ships the ack even when the pose is
        // unchanged, so client prediction can't silently diverge during the stall
        // and then pop on motion resume.
        let last_input_seq = applied.0.get(&key).copied().unwrap_or(0);
        if config.only_if_changed {
            if let Some((lp, lr, ls)) = last_sent.get(&key) {
                if *lp == pos_q && *lr == rot_packed && *ls == last_input_seq {
                    continue;
                }
            }
        }
        last_sent.insert(key, (pos_q, rot_packed, last_input_seq));
        entries.push(SnapshotEntry {
            gid: key,
            pos_q,
            rot_packed,
            lv: lv.to_array(),
            av: av.to_array(),
            last_input_seq,
        });
    }
    // Prune diff-cache + warn-set entries for despawned gids (gids are never
    // reused) so neither grows unbounded over a long-lived host with spawn/despawn
    // churn (cosim balloons, transient props, rejoining rovers).
    last_sent.retain(|k, _| live.contains(k));
    nonfinite_warned.retain(|k| live.contains(k));
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

/// Host: when a replicated entity is removed (Inspector delete, cosim teardown,
/// despawn), replicate the removal so clients despawn their local proxy instead
/// of leaving a frozen kinematic ghost pinned at its last replicated pose.
///
/// The inverse of [`broadcast_new_spawns`]. A removed entity can no longer be
/// queried for its `GlobalEntityId`, so the gid is read from a per-entity cache
/// refreshed each run from the live `NetReplicate` set — the entity is still in
/// the cache (populated the prior frame) when its removal surfaces here. Rides
/// the reliable `CommandBus`, same as `Spawn`, so a dropped despawn can't
/// resurrect the ghost.
pub fn broadcast_despawns(
    role: Res<NetworkRole>,
    mut removed: RemovedComponents<GlobalEntityId>,
    mut known: Local<HashMap<Entity, u64>>,
    q_added: Query<(Entity, &GlobalEntityId), Added<GlobalEntityId>>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if !role.is_host() {
        return;
    }
    // Maintain the Entity→gid cache INCREMENTALLY (insert on spawn) rather than
    // clearing + rebuilding it from a full query every frame: a removed entity can
    // no longer be queried for its gid, so it must be cached from when it was alive.
    // Driven by `Added<GlobalEntityId>` — the same trigger `broadcast_new_spawns`
    // uses — so a Spawn and its later Despawn are emitted symmetrically. Kept
    // self-contained (vs reusing ApiEntityRegistry) to avoid coupling to that
    // registry's removal-cleanup ordering.
    for (entity, gid) in q_added.iter() {
        known.insert(entity, gid.get());
    }
    for entity in removed.read() {
        if let Some(gid) = known.remove(&entity) {
            outbox.0.push((
                SyncChannel::CommandBus,
                SyncEnvelope::Despawn(DespawnReplicationMsg { gid }),
            ));
        }
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
    role: Res<NetworkRole>,
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
    // Only seed in a networked session. In Standalone (the default boot mode) the
    // session is LOCAL(0) and there are no peers to disambiguate against; seeding —
    // and latching `done` — there would leave a later Standalone→JoinServer client
    // stuck on color(0) (mauve), colliding with the host. Returning WITHOUT setting
    // `done` keeps the system armed for the eventual Join.
    if !role.is_networked() {
        return;
    }
    // A networked client's real session id arrives with the handshake (a few frames
    // after Join); until then `LocalSession` is LOCAL(0). Seeding now would give
    // every client color(0) — so wait. The host is authoritatively LOCAL and resolves
    // immediately, so it seeds right away (its own color legitimately *is* color(0)).
    if !role.is_host() && local.0 == SessionId::LOCAL {
        return;
    }
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
    let avatar_state = capture_avatar_state(&q_avatar, &q_reference_frames);

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
    let avatar_state = capture_avatar_state(&q_avatar, &q_reference_frames);

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

/// Capture the local avatar's pose into the positional wire tuple
/// (`translation`, `rotation`, cell coordinate, grid `ephemeris_id`) that the
/// tutor/student/share-perspective envelopes carry. Reads the first
/// `LocalAvatar` and resolves its grid's ephemeris id from the parent
/// reference frame; `None` when no local avatar exists.
///
/// CQ-112: was duplicated verbatim in `send_tutor_status_updates`,
/// `send_student_status_updates`, and `on_share_perspective`.
fn capture_avatar_state(
    q_avatar: &Query<(&Transform, &CellCoord, &ChildOf), With<LocalAvatar>>,
    q_reference_frames: &Query<&CelestialReferenceFrame>,
) -> Option<WireAvatarState> {
    q_avatar.iter().next().map(|(transform, cell, child_of)| {
        let ephem_id = q_reference_frames.get(child_of.0).ok().map(|rf| rf.ephemeris_id);
        encode_avatar_state(transform, cell, ephem_id)
    })
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
        if let Some(state) = msg.avatar_state {
            let (pos, rot, cell, grid_ephemeris_id) = decode_avatar_state(state);
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
        // The host issues its own credential — `is_authorized` now requires a
        // server-issued token (review M2), and the host trivially holds one.
        token: Some(lunco_core::ids::random_token()),
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

    // Promote only a session the SERVER already issued (it carries a server token
    // from `on_server_connected`/`setup_host_rbac`). A name alone no longer mints an
    // authenticated session — that was the token-less self-promotion of review M2.
    // An origin not in the map (or tokenless) is a profile update from a source the
    // server never authenticated; ignore it rather than fabricate authority.
    let Some(session) = rbac.sessions.get_mut(&origin.0) else {
        debug!("[net] RBAC: UpdateProfile from non-issued session {} ignored", origin.0);
        return;
    };
    if session.token.is_none() {
        warn!("[net] RBAC: session {} has no server token; refusing promotion", origin.0);
        return;
    }
    session.username = username;
    if session.role == lunco_core::session::AuthorityRole::Observer {
        session.role = lunco_core::session::AuthorityRole::Operator; // setting a name grants Operator
    }
    info!("[net] RBAC: session {} set name '{}' (role {:?})", origin.0, session.username, session.role);
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

/// Set Follow Opt-In command: local consent to be locked by a broadcasting tutor.
#[lunco_core::Command(default)]
pub struct SetFollowOptIn {
    pub enabled: bool,
}

#[lunco_core::on_command(SetFollowOptIn)]
fn on_set_follow_opt_in(
    trigger: On<SetFollowOptIn>,
    mut settings: ResMut<TutorialSettings>,
) {
    settings.follow_opt_in = trigger.event().enabled;
    // Opting out while currently following a broadcast releases the lock now,
    // rather than waiting for the tutor to flip free-movement. An explicitly
    // targeted lock is unaffected (it doesn't depend on this flag).
    if !settings.follow_opt_in {
        settings.follow_mode = false;
    }
    info!("[net] Command: Follow Opt-In set to {}", settings.follow_opt_in);
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
    let avatar_state = capture_avatar_state(&q_avatar, &q_reference_frames);

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
    on_set_follow_opt_in,
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
            .init_resource::<SessionCredential>()
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
                broadcast_despawns,
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

    #[test]
    fn despawn_envelope_roundtrips() {
        // B5: the despawn-replication envelope must survive the wire codec.
        let env = SyncEnvelope::Despawn(DespawnReplicationMsg { gid: 0x00AB_CDEF });
        let back = deserialize_env(&serialize_env(&env).expect("serialize")).expect("deserialize");
        match back {
            SyncEnvelope::Despawn(d) => assert_eq!(d.gid, 0x00AB_CDEF),
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn despawn_appended_last_keeps_discriminants_stable() {
        // The bincode codec is positional (fixint u32 discriminant, LE). `Despawn`
        // was deliberately appended LAST so it doesn't shift any pre-existing
        // variant's discriminant (which would silently break a version-skewed peer).
        // Lock that: Handshake stays at index 3, Despawn is index 11. If someone
        // re-inserts a variant mid-enum, Handshake's discriminant shifts and this
        // fails loudly.
        let handshake = serialize_env(&SyncEnvelope::Handshake(HandshakeMsg { session: 1, tick: 2, token: String::new() }))
            .expect("serialize");
        assert_eq!(&handshake[..4], &[3, 0, 0, 0], "Handshake discriminant moved");
        let despawn = serialize_env(&SyncEnvelope::Despawn(DespawnReplicationMsg { gid: 9 }))
            .expect("serialize");
        assert_eq!(&despawn[..4], &[11, 0, 0, 0], "Despawn must be the last variant");
    }

    #[test]
    fn dedup_keys_on_origin_and_op_id() {
        // B1: the dedup set is keyed on (origin, op_id), not op_id alone — two peers
        // that independently mint the same op_id (the hardcoded-seed collision) must
        // NOT clobber each other, while a true replay of the same (origin, op_id) is
        // still dropped.
        let mut dedup = SyncDedup::default();
        let (a, b, op) = (SessionId(1), SessionId(2), OpId(42));
        assert!(dedup.check_and_insert(a, op), "first (a,42) is new");
        assert!(!dedup.check_and_insert(a, op), "replay of (a,42) is a duplicate");
        assert!(dedup.check_and_insert(b, op), "same op_id from a DIFFERENT origin is NOT a dup");
        assert!(!dedup.check_and_insert(b, op), "replay of (b,42) is a duplicate");
    }

    #[test]
    fn dedup_window_is_per_peer_isolated() {
        // Per-peer windows: a chatty peer flooding op_ids must NOT evict another
        // peer's recent op from the dedup window (the global-FIFO bug). `b`'s op
        // survives `a` overflowing its own window many times over.
        let mut dedup = SyncDedup { per_origin: HashMap::new(), cap: 8 };
        let (a, b) = (SessionId(1), SessionId(2));
        assert!(dedup.check_and_insert(b, OpId(7)), "b's op is new");
        for i in 0..1000 {
            dedup.check_and_insert(a, OpId(i)); // a floods, evicting only a's own window
        }
        assert!(
            !dedup.check_and_insert(b, OpId(7)),
            "b's op still remembered despite a flooding — windows are independent"
        );
    }

    #[test]
    fn dedup_forget_drops_origin_window() {
        // On disconnect the host forgets a session's window; a same-id reconnect
        // therefore starts fresh (and its op_ids restart from new entropy anyway).
        let mut dedup = SyncDedup::default();
        let (a, op) = (SessionId(1), OpId(42));
        assert!(dedup.check_and_insert(a, op));
        assert!(!dedup.check_and_insert(a, op), "still a dup before forget");
        dedup.forget(a);
        assert!(dedup.check_and_insert(a, op), "fresh window after forget");
    }
}
