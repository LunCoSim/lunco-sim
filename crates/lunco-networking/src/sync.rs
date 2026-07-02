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

use avian3d::prelude::{AngularVelocity, LinearVelocity, PhysicsSystems, Position, RigidBody, Rotation};
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
use lunco_doc_bevy::JournalResource;
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
/// design in git history and `crates/lunco-core/src/coords.rs` rebase tests).
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

/// Low-use resources bundled so [`drain_sync_inbox`] stays within Bevy's
/// 16-param ceiling: the client's session-credential store (handshake), the clock
/// (tutor-status timestamps), and the per-command/-capability authorization policy
/// registry (relay gates).
#[derive(bevy::ecs::system::SystemParam)]
pub struct InboundClientCtx<'w> {
    credential: ResMut<'w, SessionCredential>,
    time: Res<'w, Time>,
    command_policies: Res<'w, lunco_core::session::CommandPolicyRegistry>,
    // Host-side AOI view centers, updated from inbound `ViewCenter` reports (B4 Phase 1).
    // Bundled here (vs a top-level param) to keep `drain_sync_inbox` within Bevy's
    // 16-argument system limit.
    view_centers: ResMut<'w, ViewCenters>,
    // Client-side stash of the host's scenario manifest (filled by the
    // `ScenarioManifest` arm). Bundled here for the same 16-arg-limit reason;
    // host-side arms are no-ops.
    remote_scenario: ResMut<'w, crate::scenario::RemoteScenarioManifest>,
    // Phase-3 asset transfer queues (bundled for the same 16-arg reason). The
    // arms only enqueue; the actual work runs in `crate::scenario_sync` systems.
    // Client fills `incoming_chunks` (host arm is a no-op); host fills
    // `pending_asset_requests` (client arm is a no-op).
    incoming_chunks: ResMut<'w, crate::scenario_sync::IncomingAssetChunks>,
    pending_asset_requests: ResMut<'w, crate::scenario_sync::PendingAssetRequests>,
    // Canonical Twin journal — a client applies host-sent entries here via
    // `append_remote` (merge). `Option` so the drain still runs in a build with
    // no journal (e.g. a minimal networking-only test); host arm is a no-op.
    journal: Option<Res<'w, JournalResource>>,
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

/// Client→host report of a peer's world-space view center (B4 Phase 1).
///
/// A peer that POSSESSES a vehicle has a host-derivable center (the vehicle's
/// authoritative position), so this report is only load-bearing for a FREE
/// observer flying the scene with no possession — the host has no other way to
/// know where it is looking. The client emits it from its `LocalAvatar` position
/// at `interest_hz` on the lossy `ControlStream` (a dropped report just reuses the
/// last center for one recompute; AOI hysteresis tolerates the staleness).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewCenterMsg {
    /// World-space position of the reporting peer's view (single-cell today; Phase 4
    /// carries the `CellCoord` once big_space goes multi-cell).
    pub pos: [f32; 3],
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
    // Same positional-codec rule: append after `Despawn`. Client→host only.
    ViewCenter(ViewCenterMsg),
    // ── Scenario distribution (appended LAST; see `scenario` mod) ──────────
    // Same positional-bincode rule: appended after `ViewCenter` so all prior
    // discriminants stay stable. A stale wasm client (no scenario support) vs a
    // fresh host simply doesn't send/handle these — the variants are ignored,
    // never shifted. Reserve all four now so Phase 3 (asset chunk transfer)
    // needs no further enum edit; the handlers land then.
    //
    // Host → client: the scenario manifest (CID-addressed assets + revision).
    ScenarioManifest(crate::scenario::ScenarioManifestMsg),
    // Client → host: "I'm missing these CIDs, send bytes" (Phase 3).
    AssetRequest(crate::scenario::AssetRequestMsg),
    // Host → client: one chunk of an asset's bytes (Phase 3).
    AssetChunk(crate::scenario::AssetChunkMsg),
    // Peer → host: "I already have this CID" dedupe hint (Phase 3+).
    AssetHave(crate::scenario::AssetHaveMsg),
    // Host → client: a Twin-journal entry (journal plane). Appended LAST (same
    // positional-codec rule): a stale peer that predates journal sync simply
    // never sends/handles it; prior discriminants stay put. Type owned by the
    // `journal_plane` module (the plane owns its wire shape).
    JournalEntry(crate::journal_plane::JournalEntryMsg),
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
    /// Area-of-interest recompute rate (Hz). Interest changes slowly vs pose, so
    /// this runs far below `replication_hz`.
    pub interest_hz: f32,
    /// AOI **enter** radius (world units) around a peer's view center: a body OUTSIDE
    /// a peer's interest joins it once within this distance. Pair with `aoi_exit_radius`
    /// (which must be strictly larger) for hysteresis — see that field.
    pub aoi_radius: f32,
    /// AOI **exit** radius (world units): a body already IN a peer's interest stays
    /// until it recedes past this distance. The enter<exit band is hysteresis: it
    /// stops a body hovering near the boundary from flapping in/out of interest every
    /// recompute (each flap is a re-baseline). The band must clear the fastest a body
    /// can travel between recomputes — `(exit − enter) ≥ v_max / interest_hz` — or a
    /// fast body laps the band in one tick and flaps anyway.
    pub aoi_exit_radius: f32,
    /// Force-include radius for **predicted free Dynamic bodies**. A client locally
    /// predicts nearby ownerless Dynamic bodies (pushed rocks, balloons, cosim
    /// targets); dropping their authoritative stream silently desyncs the prediction
    /// even when they sit outside the normal AOI. So any Dynamic, unowned body within
    /// this (typically ≥ `aoi_exit_radius`) radius of a peer's center is force-kept in
    /// its interest regardless of the enter/exit test. Owned bodies are force-included
    /// separately (for their owner) via the ownership table.
    pub predict_radius: f32,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            replication_hz: 20.0,
            only_if_changed: true,
            snapshot_channel: SyncChannel::ControlStream,
            interest_hz: 5.0,
            aoi_radius: 1000.0,
            aoi_exit_radius: 1500.0,
            predict_radius: 1500.0,
        }
    }
}

/// Host-side persistent replication state, populated by [`gather_snapshot`] and
/// consumed by the per-peer snapshot assembler (`assemble_and_send_snapshots`, B4
/// Phase 1). The key invariant is that `entries` holds the latest entry for
/// **every** live gid — not just this tick's changes — so the assembler can diff it
/// against each peer's last-sent digest (and seed a static-but-just-entered body with
/// its current pose). The assembler does the relevance + change decision per peer; it
/// does NOT consume `changed_this_tick` (kept only as a cheap global "moved this
/// tick" delta for observability).
#[derive(Resource, Default)]
pub struct ReplicationState {
    /// Latest snapshot entry per live gid (overwritten every tick, pruned on despawn).
    pub entries: HashMap<u64, SnapshotEntry>,
    /// gids whose `(pos, rot, ack)` changed this tick (the `only_if_changed` delta).
    /// Global observability only — per-peer send decisions live in the assembler.
    pub changed_this_tick: HashSet<u64>,
    /// Spawn catalog (`entry_id` + position) per runtime-spawned `NetSpawn` gid, so the
    /// per-peer assembler can send a `Spawn` the instant a body enters a peer's interest
    /// (B4 Phase 2 — scoped spawn). Maintained by `track_spawn_info`, pruned on despawn.
    /// Scene/Twin entities (no `NetSpawn`) are absent here — clients already hold them
    /// via scene load — so they're pose-replicated but never wire-spawned.
    pub spawn_info: HashMap<u64, SpawnReplicationMsg>,
    /// Sim tick of the latest generation (stamped onto each assembled `SnapshotMsg`).
    pub tick: u64,
    /// Bumped once per `gather_snapshot` run. The assembler runs in `Update` (the
    /// ferry clock) while generation happens in `FixedPostUpdate`; edge-detecting on
    /// this counter makes the assembler send exactly once per new generation and skip
    /// the render-throttled frames in between (where `changed_this_tick` is stale).
    pub generation: u64,
}

/// Per-session area-of-interest set: the gids the host replicates to each connected
/// peer, computed by [`recompute_interest`] and consumed by the per-peer snapshot
/// assembler (B4 Phase 1). A peer not present here (or whose set lacks a gid) simply
/// receives no pose updates for that body — with soft-exit it freezes at its last
/// pose rather than despawning.
#[derive(Resource, Default)]
pub struct PeerInterest(pub HashMap<SessionId, HashSet<u64>>);

/// Host-side latest reported world-space view center per peer, from each client's
/// [`ViewCenterMsg`]. Read by [`recompute_interest`] for FREE observers (possessing
/// peers use their vehicle position instead). Pruned when a session disconnects.
#[derive(Resource, Default)]
pub struct ViewCenters(pub HashMap<SessionId, Vec3>);

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
    command_policies: Res<lunco_core::session::CommandPolicyRegistry>,
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
        if let Err(reject) = authorize(&session_registry, &rbac, &command_policies, ev.origin, &ev.type_name, target_gid) {
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
/// - host + sender lacks the relay capability's role → `false` (caller `continue`s: not relayed, not consumed).
///
/// **Policy is data, not hardcoded.** The required role for `capability`
/// (`lunco_core::session::capability::*`) is resolved from the shared
/// [`CommandPolicyRegistry`], exactly like a reflected command goes through
/// [`authorize`]. By default these capabilities are absent from the registry and
/// resolve to [`CommandPolicy::OPEN`] (Observer floor), so any authenticated peer
/// may relay — the open-sandbox default. RBAC is introduced by registering or
/// overriding the capability's policy (e.g. tighten `SHARE_PERSPECTIVE` to
/// `Operator`), with no change to this gate.
///
/// Orthogonal protections remain regardless of role policy: the `authenticated +
/// server-issued token` credential floor (`SessionRbac::is_authorized`), the
/// per-sender identity binding done by each relay arm (anti-spoof), and the
/// per-peer follow opt-in / explicit-target consent in the `TutorStatus` arm.
#[inline]
fn authed_for_avatar_relay(
    role: &NetworkRole,
    rbac: &lunco_core::session::SessionRbac,
    policies: &lunco_core::session::CommandPolicyRegistry,
    sender: SessionId,
    capability: &str,
) -> bool {
    if !role.is_host() {
        return true;
    }
    rbac.is_authorized(sender, policies.policy_for(capability).min_role)
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
                if entities.resolve(&GlobalEntityId::from_raw(spawn.gid)).is_some() {
                    warn!("[net] Spawn rejected: gid {} already exists in registry", spawn.gid);
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
                    // Stamp this client's peer-unique journal author (journal
                    // plane) so its locally-authored edits get globally-unique
                    // entry ids — distinct from the host and other clients.
                    if let Some(journal) = ctx.journal.as_ref() {
                        journal.set_local_author(crate::journal_plane::peer_author(h.session));
                    }
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

                // Update presence info in `Presence` resource. Create the entry if the
                // peer has none yet — presence is normally seeded from `SessionProfiles`
                // (`sync_presence_with_profiles`), so a just-connected peer that hasn't
                // set a name would otherwise have its cursor silently dropped until it
                // names itself. Falls back to its profile name / "Player <id>".
                let uid = UserId(session_id.0);
                let info = presence.users.entry(uid).or_insert_with(|| PresenceInfo {
                    display_name: profiles
                        .profiles
                        .get(&session_id.0)
                        .cloned()
                        .unwrap_or_else(|| format!("Player {}", session_id.0)),
                    color: c
                        .color
                        .or_else(|| profiles.colors.get(&session_id.0).copied())
                        .unwrap_or_else(|| generate_user_color(session_id.0)),
                    active_doc: None,
                    cursor: None,
                });
                info.cursor = c.cursor;
                if let Some(color) = c.color {
                    info.color = color;
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
            SyncEnvelope::ViewCenter(vc) => {
                // Host-only: record the peer's reported free-observer center keyed on
                // the TRUSTED sender (the connection-bound session), so a peer can't
                // report a center "as" another session. Clients ignore it (the host is
                // the sole consumer — it never relays view centers).
                if role.is_host() && sender != SessionId::LOCAL {
                    ctx.view_centers.0.insert(sender, Vec3::from_array(vc.pos));
                }
            }
            SyncEnvelope::TutorStatus(mut msg) => {
                // Host authz + anti-spoof: tutor mode seizes targeted peers' avatar
                // camera and locks their input. Gate (one chokepoint) then bind the
                // tutor identity to the actual sender (mirroring the `Cursor` arm) so a
                // peer can't claim to be another session. See [`authed_for_avatar_relay`].
                if !authed_for_avatar_relay(
                    &role,
                    &rbac,
                    &ctx.command_policies,
                    sender,
                    lunco_core::session::capability::TUTOR_STATUS,
                ) {
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
                if !authed_for_avatar_relay(
                    &role,
                    &rbac,
                    &ctx.command_policies,
                    sender,
                    lunco_core::session::capability::STUDENT_STATUS,
                ) {
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
                if !authed_for_avatar_relay(
                    &role,
                    &rbac,
                    &ctx.command_policies,
                    sender,
                    lunco_core::session::capability::SHARE_PERSPECTIVE,
                ) {
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
            // ── Scenario distribution ───────────────────────────────────────
            // Phase 1: the client stashes the host's manifest; the host ignores
            // any client-sent manifest (only the host publishes the scenario).
            // Phase 3 wires AssetRequest/AssetChunk/AssetHave handlers; until
            // then they're recognized-but-no-op so the wire enum is exhaustive
            // and the reserved discriminants stay stable for a future host.
            SyncEnvelope::ScenarioManifest(m) => {
                if !role.is_host() {
                    let incoming_rev = m.revision;
                    // Dedupe on the WHOLE manifest, not `revision` alone: a
                    // scenario swap can keep byte-identical assets (⇒ identical
                    // Merkle `revision`) while changing `scenario_id` /
                    // `default_scene` / `name`. Keying only on `revision` would
                    // silently drop that swap and leave the client on the old
                    // scenario (Phase 4 would auto-load the wrong entry scene).
                    let is_new = ctx.remote_scenario.manifest.as_ref() != Some(&m);
                    if is_new {
                        info!(
                            "[net] scenario manifest received: {} assets, revision={:x?}",
                            m.assets.len(),
                            &incoming_rev[..4]
                        );
                        ctx.remote_scenario.manifest = Some(m);
                        // Phase 3 will emit AssetRequest for missing CIDs here.
                        // Phase 4 will trigger the scene load once assets land.
                    }
                }
            }
            SyncEnvelope::AssetRequest(req) => {
                // Host queues the request (keyed on the TRUSTED connection-bound
                // sender, never a wire-supplied id) for the off-thread
                // `serve_asset_requests`. Clients ignore inbound requests — only
                // the host serves scenario bytes.
                if role.is_host() && !req.missing.is_empty() {
                    ctx.pending_asset_requests.0.push((sender, req.missing));
                }
            }
            SyncEnvelope::AssetChunk(chunk) => {
                // Client queues the chunk for `reassemble_asset_chunks`. The host
                // never ingests inbound chunks (it's the sole byte source).
                if !role.is_host() {
                    ctx.incoming_chunks.0.push(chunk);
                }
            }
            SyncEnvelope::AssetHave(_) => {
                // Phase 3+: host skips streaming an asset the peer already has.
            }
            SyncEnvelope::JournalEntry(msg) => {
                // Route to the journal plane. Client mirrors the host's edit
                // history (merge via `append_remote`); host ignores inbound
                // entries in this one-way phase (client→host is bidirectional,
                // later). The plane owns the apply logic; the ferry only routes.
                if !role.is_host() {
                    if let Some(journal) = ctx.journal.as_ref() {
                        crate::journal_plane::apply_inbound_entry(journal, &msg);
                    }
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
    // B4 Phase 1: persistent per-gid state + this-tick delta. The per-peer assembler
    // (`assemble_and_send_snapshots`, in `server.rs`) reads this and does the actual
    // targeted wire send — `gather_snapshot` no longer touches `SyncOutbox`/`All`.
    mut repl: ResMut<ReplicationState>,
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
    repl.changed_this_tick.clear();
    // Fresh generation: the assembler edge-detects this to send exactly once per
    // gather, skipping the (render-throttled) `Update` frames where no new pose exists.
    repl.generation = repl.generation.wrapping_add(1);
    repl.tick = tick.0;

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
        let entry = SnapshotEntry {
            gid: key,
            pos_q,
            rot_packed,
            lv: lv.to_array(),
            av: av.to_array(),
            last_input_seq,
        };
        // Did this entity change since its last *sent* state? (the existing
        // `only_if_changed` diff). Computed before we move `entry` into the
        // persistent store below.
        let changed = !config.only_if_changed
            || last_sent
                .get(&key)
                .is_none_or(|&(lp, lr, ls)| lp != pos_q || lr != rot_packed || ls != last_input_seq);
        if changed {
            last_sent.insert(key, (pos_q, rot_packed, last_input_seq));
            repl.changed_this_tick.insert(key);
        }
        // Persist the latest entry for EVERY live gid (not just changed ones), so the
        // assembler can seed a body that's static-but-just-entered a peer's interest
        // (the soft-enter baseline). Cheap (~52 B) and decoupled from the wire.
        repl.entries.insert(key, entry);
    }
    // Prune diff-cache + warn-set entries for despawned gids (gids are never
    // reused) so neither grows unbounded over a long-lived host with spawn/despawn
    // churn (cosim balloons, transient props, rejoining rovers). `ReplicationState`
    // is pruned the same way so its persistent map tracks the live set.
    last_sent.retain(|k, _| live.contains(k));
    nonfinite_warned.retain(|k| live.contains(k));
    repl.entries.retain(|k, _| live.contains(k));
    // Phase 2: prune the spawn catalog the same way (gids are never reused). A
    // despawned NetSpawn body leaving here means the assembler will re-`Spawn` it for
    // a peer only if a NEW gid is later minted — correct, since the old proxy was
    // already torn down via `broadcast_despawns`.
    repl.spawn_info.retain(|k, _| live.contains(k));
}

/// Max `SnapshotEntry`s per `SnapshotMsg`. L2: cap so each serialized message fits in
/// ONE lightyear fragment (`FRAGMENT_SIZE` = 1180 B). A `SnapshotEntry` is ≈52 B, so
/// ~22 fit; 20 leaves headroom for the enum tag + tick + Vec length prefix. The
/// `SnapChannel` is `UnorderedUnreliable` with NO fragment retransmit, so a
/// multi-fragment message is lost wholesale if any single fragment drops (delivery
/// ≈ (1 − p_loss)^num_fragments — it gets WORSE as the scene grows). One lost datagram
/// then costs ≤20 entities for one tick (interp hides it), not every body's update.
/// The per-peer assembler (`assemble_and_send_snapshots`) chunks each peer's batch at
/// this bound.
pub const MAX_SNAPSHOT_ENTRIES: usize = 20;

/// Pure AOI interest computation — the per-peer relevance rule of [`recompute_interest`],
/// extracted so it's unit-testable without a Bevy `App`/`Time`/throttle. For each
/// session in `sessions` (excluding `LOCAL`), returns its interest gid set:
/// owned/possessed force-include ∪ spatial-with-hysteresis ∪ predicted-free-Dynamic.
/// `radius`/`exit_radius`/`predict_radius` are world-unit distances (squared inside).
/// `prev` is the last computed interest (drives hysteresis: a body already in a peer's
/// set is kept until it passes `exit_radius`; otherwise it joins within `radius`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_interest_sets(
    sessions: &[SessionId],
    positions: &HashMap<u64, Vec3>,
    dynamic: &HashSet<u64>,
    table: &[(u64, u64)],
    view_centers: &HashMap<SessionId, Vec3>,
    prev: &HashMap<SessionId, HashSet<u64>>,
    radius: f32,
    exit_radius: f32,
    predict_radius: f32,
) -> HashMap<SessionId, HashSet<u64>> {
    let r_enter2 = radius * radius;
    let r_exit2 = exit_radius * exit_radius;
    let r_pred2 = predict_radius * predict_radius;
    let all: HashSet<u64> = positions.keys().copied().collect();

    let mut next: HashMap<SessionId, HashSet<u64>> = HashMap::with_capacity(sessions.len());
    for &session in sessions {
        if session == SessionId::LOCAL {
            continue; // the host itself isn't a remote peer
        }
        let owned: Vec<u64> = table
            .iter()
            .filter_map(|&(g, s)| (s == session.0).then_some(g))
            .collect();
        // Center: possessed vehicle first (host-derivable), else the reported
        // free-observer center, else fail open to everything.
        let center = owned
            .iter()
            .find_map(|g| positions.get(g).copied())
            .or_else(|| view_centers.get(&session).copied());
        let Some(c) = center else {
            next.insert(session, all.clone()); // fail-open
            continue;
        };
        let prev_set = prev.get(&session);
        let mut set: HashSet<u64> = owned.iter().copied().collect(); // (1) force-include owned
        for (&g, &p) in positions {
            let d2 = p.distance_squared(c);
            // (2) spatial with hysteresis: enter at `radius`, keep until past `exit_radius`.
            let was_in = prev_set.is_some_and(|s| s.contains(&g));
            let threshold = if was_in { r_exit2 } else { r_enter2 };
            // (3) predicted free Dynamic: force-keep within the (wider) predict radius.
            let predict_keep = dynamic.contains(&g) && d2 <= r_pred2;
            if d2 <= threshold || predict_keep {
                set.insert(g);
            }
        }
        next.insert(session, set);
    }
    next
}

/// Pure per-peer snapshot diff — the send decision of `assemble_and_send_snapshots`,
/// extracted for testability (no `ServerMultiMessageSender`). Given a peer's interest
/// `set`, the latest `entries`, and that peer's `digest` of last-sent
/// `(pos_q, rot_packed, last_input_seq)` per gid, returns the batch to send and
/// updates `digest` in place: a body is sent if the peer lacks it (soft-enter
/// baseline) or holds a stale pose/ack; out-of-interest gids are evicted so a later
/// re-entry re-baselines (soft exit). Velocity is intentionally excluded from the key
/// (matches `gather_snapshot`'s change test).
pub(crate) fn diff_peer_batch(
    set: &HashSet<u64>,
    entries: &HashMap<u64, SnapshotEntry>,
    digest: &mut HashMap<u64, ([i32; 3], u32, u32)>,
) -> Vec<SnapshotEntry> {
    let mut batch: Vec<SnapshotEntry> = Vec::new();
    for &gid in set {
        let Some(entry) = entries.get(&gid) else {
            continue;
        };
        let key = (entry.pos_q, entry.rot_packed, entry.last_input_seq);
        if digest.get(&gid) != Some(&key) {
            batch.push(entry.clone());
            digest.insert(gid, key);
        }
    }
    digest.retain(|gid, _| set.contains(gid));
    batch
}

/// Host: recompute each connected peer's area-of-interest set (B4 **Phase 1** — this
/// now drives the wire: `assemble_and_send_snapshots` routes pose updates by it).
///
/// A peer's interest is the union of:
///   1. **Owned/possessed** gids (the authoritative `SessionRegistry` table) —
///      force-included regardless of distance, so an owner never loses its own
///      far-flung vehicle.
///   2. **Spatial** bodies within the AOI radius of the peer's view center, with
///      hysteresis: a body OUTSIDE the set joins within `aoi_radius` (enter), a body
///      ALREADY in the set stays until it recedes past `aoi_exit_radius` (exit). The
///      enter<exit band stops boundary-hovering bodies from flapping (each flap is a
///      re-baseline). Previous set comes from the last `interest.0` value.
///   3. **Predicted free Dynamic** bodies within `predict_radius` — ownerless
///      `RigidBody::Dynamic` the client locally predicts; their authoritative stream
///      can't be culled by raw distance or the prediction silently desyncs.
///
/// View center = the peer's possessed vehicle (host-derivable) if it possesses one,
/// else its last [`ViewCenterMsg`] report (free observer), else **fail open**
/// (interested in everything) so the cull can never blind a client.
pub fn recompute_interest(
    role: Res<NetworkRole>,
    config: Res<NetworkConfig>,
    time: Res<Time>,
    mut acc: Local<f32>,
    mut diag_acc: Local<f32>,
    registry: Res<SessionRegistry>,
    rbac: Res<lunco_core::session::SessionRbac>,
    view_centers: Res<ViewCenters>,
    q: Query<(&GlobalEntityId, &Transform, Option<&RigidBody>), With<NetReplicate>>,
    mut interest: ResMut<PeerInterest>,
) {
    if !role.is_host() {
        return;
    }
    *acc += time.delta_secs();
    let interval = 1.0 / config.interest_hz.max(1.0);
    if *acc < interval {
        return;
    }
    *acc = 0.0;

    // One pass over replicated bodies: world position per gid, the full live set, and
    // the ownerless-Dynamic set (predict candidates). (Single-cell space today, so
    // `Transform.translation` is the world position; Phase 4 makes this cell-relative
    // once big_space goes multi-cell.)
    let mut positions: HashMap<u64, Vec3> = HashMap::new();
    let mut dynamic: HashSet<u64> = HashSet::new();
    let table = registry.snapshot(); // hoist the ownership table once
    let owned_any: HashSet<u64> = table.iter().map(|&(g, _)| g).collect();
    for (gid, tf, rb) in q.iter() {
        let key = gid.get();
        positions.insert(key, tf.translation);
        // Predict candidate = Dynamic AND ownerless (owned Dynamic bodies are already
        // force-included for their owner; here we mean the free rocks/balloons every
        // nearby client predicts).
        if matches!(rb, Some(RigidBody::Dynamic)) && !owned_any.contains(&key) {
            dynamic.insert(key);
        }
    }
    let total = positions.len();
    let sessions: Vec<SessionId> = rbac.sessions.keys().map(|&r| SessionId(r)).collect();
    interest.0 = compute_interest_sets(
        &sessions,
        &positions,
        &dynamic,
        &table,
        &view_centers.0,
        &interest.0,
        config.aoi_radius,
        config.aoi_exit_radius,
        config.predict_radius,
    );

    // Throttled diagnostic (~5 s): how much of the broadcast AOI culls in practice.
    *diag_acc += interval;
    if *diag_acc >= 5.0 && !interest.0.is_empty() {
        *diag_acc = 0.0;
        let bodies = total;
        let total = total.max(1);
        let peers = interest.0.len();
        let avg = interest.0.values().map(|s| s.len()).sum::<usize>() as f32 / peers as f32;
        info!(
            "[net][aoi] {peers} peer(s), {bodies} replicated bodies; avg interest {avg:.0}/{total} \
             (~{:.0}% of bodies replicated per peer)",
            100.0 * avg / total as f32,
        );
    }
}

/// Host: when a runtime-spawned networked root gets its id minted, record its spawn
/// catalog (`entry_id` + position) in [`ReplicationState::spawn_info`] (B4 Phase 2).
///
/// This **no longer broadcasts** the `Spawn` to `All` — the per-peer assembler
/// (`assemble_and_send_snapshots`) sends each peer a `Spawn` the instant the body
/// enters its interest, so a cosim balloon / obstacle-field prop spawned far from a
/// player never reaches that player. `spawn_info` is the catalog the assembler reads;
/// it's pruned to the live set by `gather_snapshot`.
pub fn track_spawn_info(
    role: Res<NetworkRole>,
    q: Query<(&GlobalEntityId, &NetSpawn), Added<GlobalEntityId>>,
    mut repl: ResMut<ReplicationState>,
) {
    if !role.is_host() {
        return;
    }
    for (gid, spawn) in q.iter() {
        repl.spawn_info.insert(
            gid.get(),
            SpawnReplicationMsg {
                gid: gid.get(),
                entry_id: spawn.entry_id.clone(),
                position: spawn.position.to_array(),
            },
        );
    }
}

/// Host: when a replicated entity is removed (Inspector delete, cosim teardown,
/// despawn), replicate the removal so clients despawn their local proxy instead
/// of leaving a frozen kinematic ghost pinned at its last replicated pose.
///
/// The inverse of [`track_spawn_info`]'s catalog. A removed entity can no longer be
/// queried for its `GlobalEntityId`, so the gid is read from a per-entity cache
/// refreshed each run from the live `NetReplicate` set — the entity is still in
/// the cache (populated the prior frame) when its removal surfaces here. Rides
/// the reliable `CommandBus` to **all** peers (a despawn must reach anyone holding
/// the proxy, in- or out-of-interest), so a dropped despawn can't resurrect the ghost.
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

/// Client → host: report this peer's world-space view center for AOI culling (B4
/// Phase 1). Emitted from the `LocalAvatar` position at `interest_hz` on the lossy
/// `ControlStream`. Only a remote client reports — the host computes its own interest
/// directly (and skips itself as `SessionId::LOCAL`). A possessing peer's center is
/// host-derivable from the vehicle, but reporting unconditionally is cheap (~12 B at
/// 5 Hz) and lets the host uniformly fall back to the report when possession lapses.
pub fn send_view_center_updates(
    role: Res<NetworkRole>,
    config: Res<NetworkConfig>,
    q_avatar: Query<&Transform, With<LocalAvatar>>,
    mut timer: Local<f32>,
    mut last_sent: Local<Option<Vec3>>,
    time: Res<Time>,
    mut outbox: ResMut<SyncOutbox>,
) {
    // Host knows its own center; only a remote client needs to report.
    if !role.is_networked() || role.is_host() {
        return;
    }
    *timer += time.delta_secs();
    let interval = 1.0 / config.interest_hz.max(1.0);
    if *timer < interval {
        return;
    }
    let Some(pos) = q_avatar.iter().next().map(|tf| tf.translation) else {
        return;
    };
    // Delta gate: skip the send while the avatar is parked. 1 m floor — sub-meter
    // drift can't change AOI membership at hundred-metre radii, and recompute reuses
    // the last reported center, so a skipped report costs nothing.
    if last_sent.is_some_and(|last| pos.distance_squared(last) < 1.0) {
        return;
    }
    *timer = 0.0;
    *last_sent = Some(pos);
    outbox.0.push((
        SyncChannel::ControlStream,
        SyncEnvelope::ViewCenter(ViewCenterMsg {
            pos: pos.to_array(),
        }),
    ));
}

/// Send tutor status updates when teach_mode is enabled.
pub fn send_tutor_status_updates(
    role: Res<NetworkRole>,
    local: Res<LocalSession>,
    settings: Res<TutorialSettings>,
    // Optional: a headless (`--no-ui`) host has no workspace UI, so the
    // resource is absent. Tutor status simply reports no active document then.
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
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

    let active_doc = workspace.as_ref().and_then(|w| w.active_document);
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
    // Optional for headless hosts (see `send_tutor_status_updates`).
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
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

    let active_doc = workspace.as_ref().and_then(|w| w.active_document);
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
    // Optional for headless hosts (see `send_tutor_status_updates`).
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
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
            // Mirror active document (no-op on a headless host with no workspace)
            if let Some(ws) = workspace.as_mut() {
                if ws.active_document != tutor_status.active_doc {
                    ws.active_document = tutor_status.active_doc;
                }
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
            if let Some(ws) = workspace.as_mut() {
                if ws.active_document != tutor_status.observed_student_doc {
                    ws.active_document = tutor_status.observed_student_doc;
                }
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
        if let Some(ws) = workspace.as_mut() {
            if ws.active_document != msg.active_doc {
                ws.active_document = msg.active_doc;
            }
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
            // B4 interest management: persistent per-gid state (written by
            // `gather_snapshot`) + per-peer interest sets (written by
            // `recompute_interest`, consumed by the server-side per-peer assembler)
            // + reported free-observer view centers (from inbound `ViewCenter`).
            .init_resource::<ReplicationState>()
            .init_resource::<PeerInterest>()
            .init_resource::<ViewCenters>()
            // Scenario distribution: the client-side stash of the host's
            // manifest (filled by the `ScenarioManifest` arm of
            // `drain_sync_inbox`). Host-side publisher resource
            // (`ScenarioManifestResource`) is initialized in `setup_host` —
            // it's host-only and must not exist in single-player/client builds.
            .init_resource::<crate::scenario::RemoteScenarioManifest>()
            // Phase-3 asset transfer: the client-side download bookkeeping +
            // inbound chunk queue, and the host-side request queue. All three are
            // touched by the shared `drain_sync_inbox` (via `InboundClientCtx`) /
            // client systems, so they must exist on every peer regardless of role.
            // Host-only serve state (`HostAssetPaths`/`AssetServeTasks`) is
            // initialized in `setup_host`.
            .init_resource::<crate::scenario_sync::AssetDownloads>()
            .init_resource::<crate::scenario_sync::IncomingAssetChunks>()
            .init_resource::<crate::scenario_sync::PendingAssetRequests>()
            .init_resource::<crate::scenario_sync::AssetPersist>()
            .register_settings_section::<CursorSettings>()
            .register_settings_section::<TutorialSettings>()
            .init_resource::<SyncChannelRegistry>()
            .add_observer(apply_sync_command)
            .add_observer(on_update_profile_rbac)
            .add_systems(Startup, setup_host_rbac)
            // Journal plane: stamp the host's peer-unique journal author (clients
            // stamp theirs on handshake) so cross-peer entry ids don't collide.
            .add_systems(Startup, crate::journal_plane::stamp_host_journal_author)
            .add_systems(PreUpdate, block_bevy_inputs)
            .add_systems(Update, (
                drain_sync_inbox,
                track_spawn_info,
                broadcast_despawns,
                sync_presence_with_profiles,
                seed_local_cursor_color,
                send_local_cursor_updates,
                send_view_center_updates,
                send_tutor_status_updates,
                send_student_status_updates,
                apply_tutorial_mirroring,
                update_tutor_lifecycle,
                block_action_states,
                sync_obstacle_field_spec,
                // B4 Phase 1: compute per-peer AOI interest sets (host-only, throttled
                // to `interest_hz`). The server-side `assemble_and_send_snapshots`
                // routes pose updates by these sets.
                recompute_interest,
                // Scenario asset transfer (Phase 3), client-side: request missing
                // assets when a new manifest lands, and reassemble+persist the
                // chunks the host streams back. Both no-op on the host. The
                // host-side serve/send pair is registered in `setup_host`.
                crate::scenario_sync::request_missing_assets,
                crate::scenario_sync::reassemble_asset_chunks,
                crate::scenario_sync::drain_persist_results,
                // Journal plane: host streams new journal entries to clients.
                crate::journal_plane::broadcast_journal_entries,
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
        // Scenario-distribution commands (PromoteScenario) — its own
        // `register_commands!` set in the `scenario_sync` module.
        crate::scenario_sync::register_all_commands(app);
    }
}

#[cfg(test)]
mod codec_roundtrip {
    use super::*;
    use crate::journal_plane::JournalEntryMsg;
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
    fn scenario_variants_appended_after_viewcenter_keep_discriminants_stable() {
        // Same positional-bincode rule as `Despawn`: the four scenario variants
        // are appended after `ViewCenter` (index 12) so no prior discriminant
        // shifts. Lock the indices so a future mid-enum insert fails loudly
        // instead of silently breaking a version-skewed peer (stale wasm bundle
        // vs fresh host). ViewCenter=12, ScenarioManifest=13, AssetRequest=14,
        // AssetChunk=15, AssetHave=16.
        let viewcenter = serialize_env(&SyncEnvelope::ViewCenter(ViewCenterMsg {
            pos: [0.0; 3],
        }))
        .expect("serialize");
        assert_eq!(&viewcenter[..4], &[12, 0, 0, 0], "ViewCenter discriminant moved");

        let manifest = SyncEnvelope::ScenarioManifest(crate::scenario::ScenarioManifestMsg {
            scenario_id: [0u8; 16],
            revision: [0u8; 32],
            name: String::new(),
            default_scene: None,
            assets: Vec::new(),
        });
        let bytes = serialize_env(&manifest).expect("serialize");
        assert_eq!(&bytes[..4], &[13, 0, 0, 0], "ScenarioManifest must be index 13");

        let req = serialize_env(&SyncEnvelope::AssetRequest(crate::scenario::AssetRequestMsg {
            missing: Vec::new(),
        }))
        .expect("serialize");
        assert_eq!(&req[..4], &[14, 0, 0, 0], "AssetRequest must be index 14");

        let chunk = serialize_env(&SyncEnvelope::AssetChunk(crate::scenario::AssetChunkMsg {
            cid: Vec::new(),
            offset: 0,
            total: 0,
            data: Vec::new(),
        }))
        .expect("serialize");
        assert_eq!(&chunk[..4], &[15, 0, 0, 0], "AssetChunk must be index 15");

        let have = serialize_env(&SyncEnvelope::AssetHave(crate::scenario::AssetHaveMsg {
            cid: Vec::new(),
        }))
        .expect("serialize");
        assert_eq!(&have[..4], &[16, 0, 0, 0], "AssetHave must be index 16");

        let journal = serialize_env(&SyncEnvelope::JournalEntry(JournalEntryMsg {
            json: String::new(),
        }))
        .expect("serialize");
        assert_eq!(&journal[..4], &[17, 0, 0, 0], "JournalEntry must be index 17");
    }

    /// A `JournalEntry` round-trips through the JSON-in-bincode envelope — the
    /// `serde_json::Value` inside an `Op` (which plain bincode can't carry)
    /// survives because the entry travels as a JSON string.
    #[test]
    fn journal_entry_envelope_roundtrips() {
        use lunco_twin_journal::{
            AuthorId, AuthorTag, DomainKind, EntryId, EntryKind, JournalEntry, TwinId,
        };
        let entry = JournalEntry {
            id: EntryId { author: AuthorId::new("peer"), lamport: 7 },
            parents: vec![EntryId { author: AuthorId::local(), lamport: 3 }],
            author: AuthorTag { user: "peer".into(), tool: "remote".into() },
            at_ms: 42,
            twin: TwinId::new("t"),
            doc: lunco_doc::DocumentId::new(1),
            // The Op carries `serde_json::Value` payloads — the exact shape plain
            // bincode chokes on; the JSON-string envelope handles it.
            kind: EntryKind::Op {
                domain: DomainKind::Usd,
                op: serde_json::json!({ "SetTranslate": { "path": "/World/rover", "value": [1.0, 2.0, 3.0] } }),
                inverse: serde_json::json!({ "SetTranslate": { "path": "/World/rover", "value": [0.0, 0.0, 0.0] } }),
            },
            change_set: None,
        };
        let msg = JournalEntryMsg { json: serde_json::to_string(&entry).unwrap() };
        let bytes = serialize_env(&SyncEnvelope::JournalEntry(msg)).expect("serialize");
        let back = deserialize_env(&bytes).expect("deserialize");
        let SyncEnvelope::JournalEntry(m) = back else {
            panic!("wrong variant");
        };
        let round: JournalEntry = serde_json::from_str(&m.json).unwrap();
        assert_eq!(round.id, entry.id);
        assert_eq!(round.parents, entry.parents);
        match round.kind {
            EntryKind::Op { domain, op, .. } => {
                assert_eq!(domain, DomainKind::Usd);
                assert_eq!(op["SetTranslate"]["value"][0], 1.0);
            }
            _ => panic!("expected Op"),
        }
    }

    #[test]
    fn scenario_manifest_envelope_roundtrips() {
        use crate::scenario::{cid_for_content, scenario_revision, ScenarioAsset, ScenarioManifestMsg};
        // A realistic manifest: two assets with real CIDs + a computed revision.
        let assets = vec![
            ScenarioAsset {
                path: "scenes/main.usda".into(),
                cid: cid_for_content(b"#usda 1.0\n").to_bytes(),
                size: 10,
                media_type: Some("model/vnd.usd".into()),
            },
            ScenarioAsset {
                path: "assets/rover.glb".into(),
                cid: cid_for_content(b"glb bytes").to_bytes(),
                size: 9,
                media_type: None,
            },
        ];
        let rev = scenario_revision(&assets);
        let env = SyncEnvelope::ScenarioManifest(ScenarioManifestMsg {
            scenario_id: [0xAB; 16],
            revision: rev,
            name: "lunar_base".into(),
            default_scene: Some("scenes/main.usda".into()),
            assets,
        });
        let back = deserialize_env(&serialize_env(&env).expect("serialize")).expect("deserialize");
        match back {
            SyncEnvelope::ScenarioManifest(m) => {
                assert_eq!(m.scenario_id, [0xAB; 16]);
                assert_eq!(m.revision, rev);
                assert_eq!(m.name, "lunar_base");
                assert_eq!(m.default_scene.as_deref(), Some("scenes/main.usda"));
                assert_eq!(m.assets.len(), 2);
                // The CIDs survive the round-trip verbatim (canonical bytes).
                assert_eq!(m.assets[0].path, "scenes/main.usda");
                assert_eq!(m.assets[0].cid.len(), 36);
                assert_eq!(m.assets[1].size, 9);
            }
            _ => panic!("wrong variant after round-trip"),
        }
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

/// B4 Phase 1 verification: the AOI interest rule (`compute_interest_sets`) and the
/// per-peer snapshot diff (`diff_peer_batch`) — the two pure cores the routing flip
/// rides on. Covers the three goals the routing change must not break: correct cull,
/// owned/predict force-include surviving `R_exit`, and fail-open never blinding a peer.
#[cfg(test)]
mod aoi {
    use super::*;

    // Default radii: enter 1000, exit 1500, predict 1500 (the shipped `NetworkConfig`).
    const R_ENTER: f32 = 1000.0;
    const R_EXIT: f32 = 1500.0;
    const R_PRED: f32 = 1500.0;

    fn pos_at(x: f32) -> Vec3 {
        Vec3::new(x, 0.0, 0.0)
    }

    /// Run `compute_interest_sets` for ONE session with the given world, no hysteresis
    /// history. Returns that session's interest set.
    fn interest_for(
        session: SessionId,
        positions: &HashMap<u64, Vec3>,
        dynamic: &HashSet<u64>,
        table: &[(u64, u64)],
        view_centers: &HashMap<SessionId, Vec3>,
        radius: f32,
        exit_radius: f32,
        predict_radius: f32,
    ) -> HashSet<u64> {
        compute_interest_sets(
            &[session],
            positions,
            dynamic,
            table,
            view_centers,
            &HashMap::new(),
            radius,
            exit_radius,
            predict_radius,
        )
        .remove(&session)
        .unwrap_or_default()
    }

    #[test]
    fn culls_distant_keeps_near() {
        // Peer possesses gid 1 at the origin (its view center). gid 2 sits inside the
        // enter radius → relevant; gid 3 far outside the exit radius → culled.
        let peer = SessionId(10);
        let positions = HashMap::from([(1, pos_at(0.0)), (2, pos_at(500.0)), (3, pos_at(3000.0))]);
        let table = [(1u64, peer.0)];
        let set = interest_for(
            peer,
            &positions,
            &HashSet::new(),
            &table,
            &HashMap::new(),
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(set.contains(&1), "owned vehicle always in");
        assert!(set.contains(&2), "near body within enter radius is in");
        assert!(!set.contains(&3), "far body is culled");
    }

    #[test]
    fn owned_force_included_past_exit_radius() {
        // The peer owns TWO bodies: gid 1 at origin (becomes the center) and gid 2 far
        // beyond R_exit. Owned bodies must be force-included regardless of distance, so
        // a driver never loses its own vehicle (no reconciliation pop on return). An
        // unowned body at the same far distance is still culled.
        let peer = SessionId(10);
        let positions =
            HashMap::from([(1, pos_at(0.0)), (2, pos_at(9000.0)), (3, pos_at(9000.0))]);
        let table = [(1u64, peer.0), (2u64, peer.0)];
        let set = interest_for(
            peer,
            &positions,
            &HashSet::new(),
            &table,
            &HashMap::new(),
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(set.contains(&2), "owned-but-far body is force-included");
        assert!(!set.contains(&3), "unowned far body is culled");
    }

    #[test]
    fn hysteresis_band_depends_on_previous_membership() {
        // A body in the enter<dist<exit band (1200, between 1000 and 1500): excluded
        // when it wasn't already relevant (must cross the enter radius to join), kept
        // when it was (stays until it passes the exit radius). This is the anti-flap
        // hysteresis — the same position yields opposite membership by history.
        let peer = SessionId(10);
        let positions = HashMap::from([(1, pos_at(0.0)), (2, pos_at(1200.0))]);
        let table = [(1u64, peer.0)];

        // No history → gid 2 does not join (past enter radius 1000).
        let cold = compute_interest_sets(
            &[peer],
            &positions,
            &HashSet::new(),
            &table,
            &HashMap::new(),
            &HashMap::new(),
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(!cold[&peer].contains(&2), "outside enter radius, not previously in → excluded");

        // gid 2 already relevant → kept (1200 < exit 1500).
        let prev = HashMap::from([(peer, HashSet::from([2u64]))]);
        let warm = compute_interest_sets(
            &[peer],
            &positions,
            &HashSet::new(),
            &table,
            &HashMap::new(),
            &prev,
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(warm[&peer].contains(&2), "in-band body stays until it passes exit radius");
    }

    #[test]
    fn predicted_free_dynamic_force_included() {
        // Tight exit radius so the spatial test alone would cull gid 2 and 3 (both at
        // 1400). gid 2 is an ownerless Dynamic body (predict candidate) within the
        // predict radius → force-kept; gid 3 (non-dynamic) → culled.
        let peer = SessionId(10);
        let positions =
            HashMap::from([(1, pos_at(0.0)), (2, pos_at(1400.0)), (3, pos_at(1400.0))]);
        let table = [(1u64, peer.0)];
        let dynamic = HashSet::from([2u64]);
        let set = interest_for(
            peer,
            &positions,
            &dynamic,
            &table,
            &HashMap::new(),
            R_ENTER,
            1100.0, // exit radius below 1400 so spatial can't keep these
            R_PRED, // 1500 > 1400
        );
        assert!(set.contains(&2), "predicted free Dynamic within predict radius is force-kept");
        assert!(!set.contains(&3), "non-dynamic body at same distance is culled");
    }

    #[test]
    fn no_center_fails_open() {
        // Peer owns nothing and never reported a view center → interested in EVERY live
        // body (fail-open), so the cull can never blind a client.
        let peer = SessionId(10);
        let positions = HashMap::from([(1, pos_at(0.0)), (2, pos_at(9000.0)), (3, pos_at(50.0))]);
        let set = interest_for(
            peer,
            &positions,
            &HashSet::new(),
            &[], // owns nothing
            &HashMap::new(), // no report
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert_eq!(set, positions.keys().copied().collect::<HashSet<_>>(), "fail-open = all bodies");
    }

    #[test]
    fn reported_view_center_drives_cull_for_free_observer() {
        // A non-possessing observer reports a view center at x=5000. Bodies are culled
        // relative to THAT, not the origin: gid 2 near the report is in, gid 3 near the
        // origin (far from the report) is out.
        let peer = SessionId(10);
        let positions = HashMap::from([(2, pos_at(5200.0)), (3, pos_at(0.0))]);
        let view_centers = HashMap::from([(peer, pos_at(5000.0))]);
        let set = interest_for(
            peer,
            &positions,
            &HashSet::new(),
            &[], // owns nothing → uses the report
            &view_centers,
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(set.contains(&2), "body near the reported center is relevant");
        assert!(!set.contains(&3), "body far from the reported center is culled");
    }

    #[test]
    fn local_session_excluded() {
        // The host's own LOCAL session is never a remote peer and gets no interest set.
        let out = compute_interest_sets(
            &[SessionId::LOCAL, SessionId(10)],
            &HashMap::from([(1, pos_at(0.0))]),
            &HashSet::new(),
            &[(1u64, 10)],
            &HashMap::new(),
            &HashMap::new(),
            R_ENTER,
            R_EXIT,
            R_PRED,
        );
        assert!(!out.contains_key(&SessionId::LOCAL), "LOCAL is skipped");
        assert!(out.contains_key(&SessionId(10)), "remote peer is present");
    }

    // ── diff_peer_batch (per-peer send decision) ──────────────────────────────

    fn entry(gid: u64, x: i32, seq: u32) -> SnapshotEntry {
        SnapshotEntry {
            gid,
            pos_q: [x, 0, 0],
            rot_packed: 0,
            lv: [0.0; 3],
            av: [0.0; 3],
            last_input_seq: seq,
        }
    }

    #[test]
    fn diff_sends_soft_enter_baseline_then_stays_quiet() {
        let entries = HashMap::from([(1u64, entry(1, 10, 0)), (2u64, entry(2, 20, 0))]);
        let set = HashSet::from([1u64, 2u64]);
        let mut digest = HashMap::new();

        // First assemble: peer has nothing → both sent (soft-enter baseline).
        let first = diff_peer_batch(&set, &entries, &mut digest);
        assert_eq!(first.len(), 2, "both bodies sent as baseline");

        // Nothing changed → empty batch (no redundant resend).
        let second = diff_peer_batch(&set, &entries, &mut digest);
        assert!(second.is_empty(), "unchanged bodies are not re-sent");
    }

    #[test]
    fn diff_resends_only_changed_pose_or_ack() {
        let mut entries = HashMap::from([(1u64, entry(1, 10, 0)), (2u64, entry(2, 20, 0))]);
        let set = HashSet::from([1u64, 2u64]);
        let mut digest = HashMap::new();
        let _ = diff_peer_batch(&set, &entries, &mut digest); // prime

        // gid 1 moves; gid 2's ack advances while its pose is unchanged (stall ack).
        entries.insert(1, entry(1, 11, 0));
        entries.insert(2, entry(2, 20, 5));
        let batch = diff_peer_batch(&set, &entries, &mut digest);
        let gids: HashSet<u64> = batch.iter().map(|e| e.gid).collect();
        assert_eq!(gids, HashSet::from([1, 2]), "moved body AND ack-advanced body both resent");

        // Stable again → quiet.
        assert!(diff_peer_batch(&set, &entries, &mut digest).is_empty());
    }

    #[test]
    fn diff_soft_exit_evicts_then_re_enters_with_baseline() {
        let entries = HashMap::from([(1u64, entry(1, 10, 0)), (2u64, entry(2, 20, 0))]);
        let mut digest = HashMap::new();
        let _ = diff_peer_batch(&HashSet::from([1u64, 2u64]), &entries, &mut digest); // both known

        // gid 2 leaves interest → not sent, evicted from the digest (soft exit).
        let only1 = HashSet::from([1u64]);
        let exit_batch = diff_peer_batch(&only1, &entries, &mut digest);
        assert!(exit_batch.is_empty(), "soft exit sends nothing");
        assert!(!digest.contains_key(&2), "exited body evicted from digest");

        // gid 2 re-enters (unchanged pose) → re-sent as a fresh baseline, NOT silently
        // skipped (its client proxy was frozen and may need re-seeding).
        let both = HashSet::from([1u64, 2u64]);
        let reenter = diff_peer_batch(&both, &entries, &mut digest);
        assert_eq!(reenter.iter().map(|e| e.gid).collect::<Vec<_>>(), vec![2], "re-entry re-baselines gid 2");
    }
}
