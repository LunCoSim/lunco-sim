//! Scenario **asset transfer** — Phase 3 of scenario distribution (the bytes).
//!
//! Phase 1 ([`lunco_networking_scenario`]) publishes the manifest: "scenario X at revision
//! R with these asset CIDs". This module moves the actual **bytes**, one-way
//! host → client, so a joined client can materialise the scenario in its local
//! cache (`<cache_dir>/scenarios/<scenario_id>/<revision>/<path>`). It is deliberately the
//! *content plane* only — opaque bytes addressed by CID, verified by re-hashing,
//! no merge (documents merge via the journal; see `NETWORKING_ASSET_SYNC_DESIGN.md`).
//!
//! Flow:
//! - **client** ([`request_missing_assets`]): when a new manifest lands, diff its
//!   asset CIDs against what we've already fetched this session and emit one
//!   [`AssetRequestMsg`](lunco_networking_scenario::AssetRequestMsg) for the missing set on
//!   the reliable [`SyncChannel::BulkData`] lane.
//! - **host** ([`serve_asset_requests`]): a client's request is queued by the
//!   inbox drain into [`PendingAssetRequests`]; this system resolves each CID to
//!   its on-disk path ([`HostAssetPaths`], filled when the manifest builds) and
//!   spawns an **off-thread** read+chunk task (whole-file reads must not stall the
//!   `Update` ferry — same reason the manifest build is off-thread). The per-peer
//!   SEND of the produced chunks lives in `server.rs` (it needs lightyear's
//!   `ServerMultiMessageSender`).
//! - **client** ([`reassemble_asset_chunks`]): chunks queued by the inbox drain
//!   into [`IncomingAssetChunks`] are reassembled per CID (the ordered-reliable
//!   `BulkChannel` guarantees in-order arrival per asset), verified by re-hashing
//!   to the CID (**fail-closed** — a mismatched blob is discarded, never cached),
//!   then persisted via `lunco_storage::write_file_sync`.
//!
//! Persisted revision caches and their admission catalog are application-owned.
//! Transfer and scene publication remain pinned to the live connection/mount.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use crossbeam_channel::{Receiver, Sender, unbounded};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use lunco_command_contracts::{SessionId, SyncChannel};
use lunco_core_session::NetworkRole;
use lunco_storage::StorageHandle;

use crate::sync::{SyncEnvelope, SyncOutbox};
use lunco_networking_scenario::{
    AssetChunkMsg, AssetRequestMsg, ScenarioJournalHead, ScenarioManifestMsg, cid_from_bytes,
};

/// Convert the journal runtime's identity into the scenario wire contract at
/// the sync boundary. The protocol package does not depend on the journal.
pub fn scenario_journal_head(entry: &lunco_twin_journal::EntryId) -> ScenarioJournalHead {
    ScenarioJournalHead {
        author: entry.author.0.clone(),
        lamport: entry.lamport,
    }
}

/// Convert a received scenario wire position into the journal identity used by
/// replay and ordering code. Callers use it only at the journal edge.
pub fn journal_entry_id(head: &ScenarioJournalHead) -> lunco_twin_journal::EntryId {
    lunco_twin_journal::EntryId {
        author: lunco_twin_journal::AuthorId::new(head.author.clone()),
        lamport: head.lamport,
    }
}

/// Convert the optional journal position carried by a manifest at the sync
/// boundary. `None` means the snapshot has no journal base.
pub fn manifest_journal_head(
    manifest: Option<&ScenarioManifestMsg>,
) -> Option<lunco_twin_journal::EntryId> {
    manifest
        .and_then(|manifest| manifest.journal_head.as_ref())
        .map(journal_entry_id)
}

/// Host-side scenario state shared by the manifest builder and transport adapter.
///
/// The manifest wire shape lives in [`lunco_networking_scenario`]. This resource
/// stays in the Bevy synchronization runtime because it is ECS state, not a
/// transport-neutral message contract.
#[derive(Resource, Default, Clone, Debug)]
pub struct ScenarioManifestResource {
    pub owner: Option<lunco_workspace::TwinId>,
    /// The current scenario manifest. `None` until the host opens a Twin/scene.
    pub manifest: Option<ScenarioManifestMsg>,
}

/// Client-side scenario state populated by the synchronization inbox.
///
/// The manifest wire shape lives in [`lunco_networking_scenario`]. Keeping this
/// resource here prevents the contract crate from depending on Bevy.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct RemoteScenarioManifest {
    pub connection: Option<Entity>,
    pub host_twin: Option<lunco_workspace::TwinId>,
    /// Catalog ordering pinned when this manifest was admitted.
    pub cache_admission: Option<CacheAdmission>,
    /// The most recent manifest the host pushed.
    pub manifest: Option<ScenarioManifestMsg>,
}

/// Immutable catalog admission identity, independent of worker completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheAdmission {
    pub cached_at_unix_ns: u64,
    pub token: String,
}

impl CacheAdmission {
    fn new() -> Result<Self, String> {
        let timestamp = web_time::SystemTime::now()
            .duration_since(web_time::SystemTime::UNIX_EPOCH)
            .map_err(|error| format!("invalid cache admission clock: {error}"))?;
        let cached_at_unix_ns = u64::try_from(timestamp.as_nanos())
            .map_err(|_| "cache admission timestamp exceeds its storage range".to_string())?;
        Ok(Self {
            cached_at_unix_ns,
            token: lunco_id::random_token(),
        })
    }
}

impl RemoteScenarioManifest {
    pub fn is_live(&self, role: NetworkRole, connection: Option<Entity>) -> bool {
        role == NetworkRole::Client
            && connection.is_some()
            && self.connection == connection
            && self.host_twin.is_some()
            && self.manifest.is_some()
    }
}

/// One owner for client scenario admission and its asynchronous result channels.
/// Replacing channels disconnects old senders, so retired work cannot publish
/// into a newer download tally even if its cache write has already started.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ClientScenarioLifecycle<'w> {
    pub remote: ResMut<'w, RemoteScenarioManifest>,
    pub handshake: ResMut<'w, HandshakenConnection>,
    downloads: ResMut<'w, AssetDownloads>,
    persist: ResMut<'w, AssetPersist>,
    probe: ResMut<'w, AssetCacheProbe>,
    probe_state: ResMut<'w, CacheProbeState>,
    fetch: ResMut<'w, crate::http_fetch::AssetHttpFetch>,
    pub incoming: ResMut<'w, IncomingAssetChunks>,
    status: ResMut<'w, ScenarioDownloadStatus>,
    pub spawns: ResMut<'w, lunco_core_session::PendingReplicatedSpawns>,
    pub scene: ResMut<'w, lunco_core_session::ReplicatedScene>,
    pub snapshots: ResMut<'w, lunco_networking_core::session::IncomingSnapshots>,
    prediction: lunco_networking_core::prediction::PredictionStateLifecycle<'w>,
    pub deferred: ResMut<'w, DeferredSceneMessages>,
    pub journal: ResMut<'w, crate::journal_plane::ReplicatedJournal>,
    roots: Option<Res<'w, lunco_assets_core::TwinRoots>>,
}

impl ClientScenarioLifecycle<'_> {
    fn reset_downloads(&mut self) {
        *self.downloads = AssetDownloads::default();
        *self.persist = AssetPersist::default();
        *self.probe = AssetCacheProbe::default();
        *self.probe_state = CacheProbeState::default();
        *self.fetch = crate::http_fetch::AssetHttpFetch::default();
        self.incoming.0.clear();
        *self.status = ScenarioDownloadStatus::default();
    }

    fn clear_content(&mut self) {
        self.snapshots.0.clear();
        self.prediction.reset();
        if let Some(scene) = self.scene.0.take() {
            if scene.owns_mount {
                match self.roots.as_ref() {
                    Some(roots) => {
                        if let Err(error) = roots.unregister_name(&scene.authority) {
                            error!("[net] could not retire downloaded Twin mount: {error}");
                        }
                    }
                    None => {
                        error!("[net] cannot retire downloaded Twin mount without its asset owner")
                    }
                }
            }
        }
        *self.remote = RemoteScenarioManifest::default();
        self.reset_downloads();
    }

    pub(crate) fn withdraw(&mut self) {
        if let (Some(connection), Some(owner)) = (self.remote.connection, self.remote.host_twin) {
            self.spawns.retire(connection, owner);
            self.journal
                .retire(lunco_core_session::ReplicationScope::Twin(owner));
            self.deferred.retire_scope(crate::scope::wire_scope(
                lunco_core_session::ReplicationScope::Twin(owner),
            ));
        }
        if let Some(scene) = self.scene.0.as_ref() {
            self.spawns.retire(scene.connection, scene.host_twin);
        }
        self.clear_content();
    }

    pub(crate) fn replace_manifest(
        &mut self,
        connection: Entity,
        owner: lunco_workspace::TwinId,
        manifest: ScenarioManifestMsg,
    ) {
        let cache_admission = match CacheAdmission::new() {
            Ok(admission) => Some(admission),
            Err(error) => {
                warn!("[net] scenario cache catalog admission rejected: {error}");
                None
            }
        };
        if self.remote.connection == Some(connection) && self.remote.host_twin == Some(owner) {
            self.clear_content();
        } else {
            self.withdraw();
        }
        self.remote.connection = Some(connection);
        self.remote.host_twin = Some(owner);
        self.remote.manifest = Some(manifest);
        self.remote.cache_admission = cache_admission;
    }
}

pub(crate) fn reconcile_client_scenario_owner(
    role: Res<NetworkRole>,
    connection: Res<lunco_core_session::ClientConnection>,
    mut state: ClientScenarioLifecycle,
    mut inbox: ResMut<crate::sync::SyncInbox>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if role.is_changed()
        || connection.is_changed()
        || (state.remote.connection.is_some() && state.remote.connection != connection.0)
    {
        state.withdraw();
        state.spawns.clear();
        state.journal.clear();
        state.handshake.0 = None;
        *state.deferred = DeferredSceneMessages::default();
        state.snapshots.0.clear();
        inbox.entries.clear();
        inbox.connection = connection.0;
        if !role.is_host() {
            outbox.0.clear();
        }
    }
}

pub(crate) fn on_twin_closed_client(
    event: On<lunco_workspace::TwinClosed>,
    mut state: ClientScenarioLifecycle,
) {
    if state
        .scene
        .0
        .as_ref()
        .is_some_and(|owner| owner.root == event.root)
    {
        state.withdraw();
    }
}

// ── In-session chunk transfer: the FALLBACK bytes path ───────────────────────
//
// Used only when the host advertises no `asset_base_url` (no `transport-http`, or
// `LUNCO_ASSET_PORT=0`). The primary path is `crate::http_fetch`. Keep this one for
// small scenarios and for hosts with no HTTP surface; do NOT push a large twin
// through it (see `MAX_CHUNKS_PER_FRAME`).

/// Asset chunk payload size (bytes). Sized so an `AssetChunk` envelope fits in ONE
/// lightyear packet (`MAX_PACKET_SIZE` = 1200 B, minus packet header + fragment
/// metadata + our bincode envelope), so a chunk never multiplies the per-message
/// reliable-ack bookkeeping by fragmenting.
///
/// A bigger chunk does NOT fail on size — lightyear fragments it — but it lets one
/// frame queue tens of MB into the unbounded `unacked_messages` buffer, which
/// saturates the link and stalls delivery outright (that is what motivated the HTTP
/// bytes plane).
pub const ASSET_CHUNK_SIZE: usize = 1024;

/// Max asset chunks the host flushes to the wire per frame.
///
/// NOT real backpressure — it rate-limits *queueing*, not *delivery*. lightyear's
/// reliable `buffer_send` never rejects: every chunk lands in `unacked_messages` and
/// is resent until acked. So a transfer larger than the link can drain still grows
/// the backlog without bound and eventually wedges (measured: a 40 MB twin stalls
/// the client at ~12 MB while the host's queue climbs past 27 k chunks). That is a
/// property of this path, not a tuning problem — which is why the bytes plane moved
/// to HTTP (`crate::http_fetch`), where the OS provides flow control.
///
/// TODO(flow-control): if this path ever needs to carry large scenarios, gate it on
/// chunks actually *received* — the reserved `AssetHave` envelope is the ack.
pub const MAX_CHUNKS_PER_FRAME: usize = 32;

/// Sender-side high-water mark: max chunks we ESTIMATE lightyear still holds
/// unacked for one peer before the ferry stops enqueuing to that peer. This is
/// the missing half of [`MAX_CHUNKS_PER_FRAME`]: the per-frame cap rate-limits
/// queueing, but for a stalled client the queue still grew without bound (the
/// measured 27 k-chunk wedge). At ~1 KiB per chunk this bounds the per-peer
/// reliable backlog to ~1 MiB; undelivered chunks wait in the host's inert
/// `ready` buffer instead of lightyear's resend bookkeeping.
///
/// lightyear exposes no unacked count and the wire has no chunk ack yet (the
/// reserved `AssetHave` envelope — see the TODO above), so the estimate is
/// sends minus an assumed drain ([`ASSUMED_DRAIN_CHUNKS_PER_SEC`]).
pub const MAX_UNACKED_CHUNK_ESTIMATE: f32 = 1024.0;

/// Assumed per-peer delivery rate used to decay the unacked estimate
/// (chunks/second; ~256 KiB/s at [`ASSET_CHUNK_SIZE`]). A transfer within the
/// [`MAX_UNACKED_CHUNK_ESTIMATE`] budget (~1 MiB — this path's intended small-
/// scenario case) is never throttled; beyond it, enqueueing converges to this
/// rate whether the client is draining or stalled — conservative on purpose,
/// since without an ack we cannot tell the two apart, and large transfers
/// belong on the HTTP bytes plane anyway (see the module docs).
pub const ASSUMED_DRAIN_CHUNKS_PER_SEC: f32 = 256.0;

/// Hard cap on a single asset offer's byte size, enforced at all three points of
/// the offer path (client send, host inbox drain, host ingest) so no single
/// unchunked `AssetOffer` can balloon a peer's memory. 96 MiB: the legitimate
/// large case is a client-side DEM import (~40 MB GeoTIFF) with headroom; the
/// long-term path for anything bigger is chunking offers like `AssetChunkMsg`
/// (the `TODO(bidirectional-content)` in `scenario.rs`). Oversized offers are
/// rejected with a `warn!`, never a panic.
pub const MAX_ASSET_OFFER_BYTES: usize = 96 * 1024 * 1024;

// ── Resources ───────────────────────────────────────────────────────────────

/// Client-side: in-flight + completed download bookkeeping.
#[derive(Resource, Default)]
pub struct AssetDownloads {
    /// Per-CID reassembly buffers for assets still arriving.
    inflight: HashMap<Vec<u8>, Inflight>,
    /// CIDs already requested (or found cached) this session, so a repeated
    /// manifest change doesn't re-emit a request for the same asset. Cleared for
    /// a CID if its download fails verification, so a fresh manifest can retry.
    requested: HashSet<Vec<u8>>,
    /// CIDs downloaded, verified, and persisted to the cache this session. Drives
    /// [`Self::all_cached`] — the Phase-4 "scene is ready to load" signal.
    completed: HashSet<Vec<u8>>,
    /// Outstanding write count per verified CID. A CID that appears at N manifest
    /// paths (byte-identical files share one content id) spawns N writes; it only
    /// counts as `completed` once all N report success.
    pending_writes: HashMap<Vec<u8>, usize>,
}

impl AssetDownloads {
    /// True once **every** asset CID in `manifest` has been downloaded, verified,
    /// and persisted — i.e. the entry scene and all its co-located refs are on
    /// disk/OPFS and a [`mount_scenario_twin`] load will resolve. `false` for an
    /// empty manifest (nothing to consume).
    pub fn all_cached(&self, manifest: &ScenarioManifestMsg) -> bool {
        !manifest.assets.is_empty()
            && manifest
                .assets
                .iter()
                .all(|a| self.completed.contains(&a.cid))
    }

    /// Has this CID already been requested (or found cached) this session?
    pub(crate) fn is_requested(&self, cid: &[u8]) -> bool {
        self.requested.contains(cid)
    }

    /// Claim a CID so a second transport (or a later frame) won't re-fetch it.
    pub(crate) fn mark_requested(&mut self, cid: Vec<u8>) {
        self.requested.insert(cid);
    }

    /// Release a CID after a failed fetch/verify/write so a fresh manifest retries it.
    pub(crate) fn forget_requested(&mut self, cid: &[u8]) {
        self.pending_writes.remove(cid);
        self.requested.remove(cid);
    }

    /// Record that a verified CID owes `n` cache writes (one per manifest path that
    /// carries it); it only counts as `completed` once all of them report success.
    pub(crate) fn expect_writes(&mut self, cid: Vec<u8>, n: usize) {
        self.pending_writes.insert(cid, n);
    }
}

/// UI-facing download progress for the in-flight scenario sync (G2). Updated by
/// [`update_scenario_download_status`] from [`AssetDownloads`] + the remote
/// manifest; rendered by the sandbox's progress overlay (mirrors
/// `terrain_progress`). `active` goes false once [`AssetDownloads::all_cached`].
#[derive(Resource, Default, Clone)]
pub struct ScenarioDownloadStatus {
    pub active: bool,
    pub name: String,
    pub assets_done: usize,
    pub assets_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

impl ScenarioDownloadStatus {
    /// `0.0..=1.0` while the total is known; `None` when there is nothing to fetch.
    pub fn fraction(&self) -> Option<f32> {
        (self.bytes_total > 0)
            .then(|| (self.bytes_done as f32 / self.bytes_total as f32).clamp(0.0, 1.0))
    }
}

#[derive(Default)]
struct Inflight {
    total: u64,
    buf: Vec<u8>,
    /// Running SHA-256 fed one chunk at a time, so verification costs nothing
    /// extra at completion (no full-buffer re-hash) and the CPU is spread across
    /// the download instead of a single main-thread spike — identical on native
    /// and web (the key to not blocking the browser main thread on a big asset).
    hasher: Sha256,
}

/// Async persist outcome, sent from the spawned write future back to
/// [`drain_persist_results`]. Uniform across platforms — native pushes from an
/// `AsyncComputeTaskPool` task, web from a `spawn_local` future.
pub(crate) struct PersistOutcome {
    cid: Vec<u8>,
    ok: bool,
}

/// Client-side channel carrying async persist outcomes. A resource so the
/// spawned write future (which outlives the submitting system) can report back.
#[derive(Resource)]
pub struct AssetPersist {
    pub(crate) tx: Sender<PersistOutcome>,
    rx: Receiver<PersistOutcome>,
}

impl Default for AssetPersist {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

/// Async cache-probe outcome: the manifest CIDs already present **and
/// CID-verified** in the local scenario cache. Sent from the spawned probe future
/// back to [`drive_cache_probe`], which marks them `completed`+`requested` so
/// [`request_missing_assets`] skips them instead of re-fetching.
struct ProbeOutcome {
    revision: [u8; 32],
    cached: HashSet<Vec<u8>>,
}

/// Client-side channel carrying async cache-probe outcomes (sibling of
/// [`AssetPersist`]). A resource so the spawned probe future — which outlives the
/// kicking system — can report back, uniform native (`AsyncComputeTaskPool`) /
/// web (`spawn_local`).
#[derive(Resource)]
pub struct AssetCacheProbe {
    tx: Sender<ProbeOutcome>,
    rx: Receiver<ProbeOutcome>,
}

impl Default for AssetCacheProbe {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

/// Cache-probe coordination: `kicked` = manifest revision a probe was launched for;
/// `settled` = revision whose results have been applied to [`AssetDownloads`].
/// [`request_missing_assets`] waits for `settled` to match the current manifest
/// revision before emitting any request — closing the race between the sync system
/// and the async probe (which otherwise lands a frame or two after the manifest
/// change).
#[derive(Resource, Default)]
pub struct CacheProbeState {
    kicked: Option<[u8; 32]>,
    settled: Option<[u8; 32]>,
}

impl CacheProbeState {
    /// True once the cross-session cache probe has reported for `revision` — the
    /// point at which "not in `completed`" reliably means "we really must fetch it".
    pub(crate) fn settled_for(&self, revision: [u8; 32]) -> bool {
        self.settled == Some(revision)
    }
}

// ── Cross-session cache index (G1 integrity + G3 menu metadata) ───────────────

/// One per-asset record persisted in an immutable catalog admission. The probe keys
/// cache-hits on `cid` — not file presence — so a twin whose content changed at a
/// path is re-fetched, never served stale; the cached-twin menu reads the same
/// file for name/size/scene.
#[derive(serde::Serialize, serde::Deserialize)]
struct ScenarioIndexAsset {
    path: String,
    cid: Vec<u8>,
    size: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ScenarioIndex {
    summary: CachedTwinSummary,
    assets: Vec<ScenarioIndexAsset>,
}

fn catalog_record_name(admission: &CacheAdmission) -> String {
    format!(
        ".scenario-{:020}-{}.json",
        admission.cached_at_unix_ns, admission.token
    )
}

/// One immutable downloaded revision admission. The catalog selects the newest
/// captured admission per UUID, never the last worker to finish.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CachedTwinSummary {
    pub scenario_id: [u8; 16],
    pub name: String,
    pub default_scene: Option<String>,
    pub total_bytes: u64,
    pub revision: [u8; 32],
    pub cached_at_unix_ns: u64,
    pub admission_token: String,
}

/// Cache catalog processing and metadata retention budgets. Asset cache bytes
/// are retained independently; these limits bound catalog admission/read work.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ScenarioCacheLimits {
    pub max_namespace_entries: usize,
    pub max_admission_records: usize,
    pub max_record_bytes: usize,
}

impl Default for ScenarioCacheLimits {
    fn default() -> Self {
        Self {
            max_namespace_entries: 4096,
            max_admission_records: 8,
            max_record_bytes: crate::codec::MAX_ENVELOPE_BYTES,
        }
    }
}

#[derive(Resource, Default)]
pub struct CachedTwinsRegistry {
    pub entries: Vec<CachedTwinSummary>,
}

impl CachedTwinsRegistry {
    fn admit(&mut self, summary: CachedTwinSummary) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.scenario_id == summary.scenario_id)
        {
            if catalog_order(existing) >= catalog_order(&summary) {
                return;
            }
            *existing = summary;
        } else {
            self.entries.push(summary);
        }
        self.entries.sort_by(|a, b| {
            catalog_order(b)
                .cmp(&catalog_order(a))
                .then(a.scenario_id.cmp(&b.scenario_id))
        });
    }
}

fn catalog_order(summary: &CachedTwinSummary) -> (u64, &str) {
    (summary.cached_at_unix_ns, &summary.admission_token)
}

/// Application-owned result channel for the asynchronous boot catalog scan.
#[derive(Resource)]
pub struct CachedTwinsIndex {
    tx: Sender<Vec<CachedTwinSummary>>,
    rx: Receiver<Vec<CachedTwinSummary>>,
}

impl Default for CachedTwinsIndex {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx }
    }
}

/// Client-side queue: raw chunks pushed by the `AssetChunk` arm of
/// `drain_sync_inbox`, drained by [`reassemble_asset_chunks`]. Bundled into the
/// inbox drain via `InboundClientCtx` (16-param ceiling) like the manifest stash.
#[derive(Resource, Default)]
pub struct IncomingAssetChunks(pub Vec<AssetChunkMsg>);

/// Host-side queue: `(requesting session, missing CIDs)` pushed by the
/// `AssetRequest` arm of `drain_sync_inbox`, drained by [`serve_asset_requests`].
#[derive(Resource, Default)]
pub struct PendingAssetRequests(pub Vec<(SessionId, Vec<Vec<u8>>)>);

/// Host-side queue: assets a client **offered** (imported into the shared twin),
/// pushed by the `AssetOffer` arm of `drain_sync_inbox`, drained by the host's
/// `ingest_asset_offers` (`server.rs`) — verify-write-to-twin then rebuild the
/// manifest so the import redistributes. Keyed on the TRUSTED connection-bound
/// sender (like [`PendingAssetRequests`]) so a disconnect can sweep its queued
/// offers before they're ingested.
#[derive(Resource, Default)]
pub struct PendingAssetOffers(pub Vec<(SessionId, lunco_networking_scenario::AssetOfferMsg)>);

/// Client → host: offer an asset the local peer just imported so the host writes it
/// into the shared twin and redistributes it (the bidirectional counterpart of the
/// host serve). Computes the CID locally; the host re-verifies. Pushes onto the
/// [`SyncOutbox`] over the reliable `BulkData` lane. No-op if the payload is empty.
///
/// TODO(bidirectional-content): wire the call site — fire this from the actual
/// import surface (file-open / drag-drop / palette add) when a NEW asset enters the
/// twin. Today it's the mechanism, not yet the trigger. Also cap `bytes` and chunk
/// large offers (see [`AssetOfferMsg`](lunco_networking_scenario::AssetOfferMsg)).
pub fn offer_asset_to_host(
    outbox: &mut crate::sync::SyncOutbox,
    path: impl Into<String>,
    bytes: Vec<u8>,
) {
    if bytes.is_empty() {
        return;
    }
    if bytes.len() > MAX_ASSET_OFFER_BYTES {
        warn!(
            "[net] asset offer of {} bytes exceeds the {} byte cap; not sending (chunked offers are the TODO path)",
            bytes.len(),
            MAX_ASSET_OFFER_BYTES
        );
        return;
    }
    let cid = lunco_networking_scenario::cid_for_content(&bytes).to_bytes();
    outbox.0.push((
        lunco_command_contracts::SyncChannel::BulkData,
        crate::sync::SyncEnvelope::AssetOffer(lunco_networking_scenario::AssetOfferMsg {
            path: path.into(),
            cid,
            data: bytes,
        }),
    ));
}

/// Host-side: CID → absolute on-disk path for every asset in the current
/// scenario, filled when the off-thread manifest build completes
/// (`drive_scenario_manifest`). The request server reads bytes through this map
/// rather than re-walking the Twin.
#[derive(Resource, Default)]
pub struct HostAssetPaths(pub HashMap<Vec<u8>, PathBuf>);

/// Host-side: in-flight off-thread read+chunk jobs, each tagged with the session
/// that requested them. Polled + sent per-peer by `server.rs`.
#[derive(Resource, Default)]
pub struct AssetServeTasks(pub Vec<(SessionId, Task<Vec<AssetChunkMsg>>)>);

// ── Cache paths ───────────────────────────────────────────────────────────────

/// Immutable revision cache root: `<cache_dir>/scenarios/<hex id>/<hex revision>/`.
pub fn scenario_cache_root(scenario_id: &[u8; 16], revision: &[u8; 32]) -> PathBuf {
    lunco_assets_core::scenarios_dir()
        .join(hex_bytes(scenario_id))
        .join(hex_bytes(revision))
}

/// A safe *relative* `PathBuf` from a `/`-separated manifest asset path,
/// **rejecting traversal** (empty / `.` / `..` / backslash segments) — the path
/// comes from a remote host and must never escape a target root. `None` if unsafe
/// or empty.
pub fn safe_rel_path(rel: &str) -> Option<PathBuf> {
    if !lunco_assets_path::is_safe_relative_path(rel) {
        warn!("[net] rejecting unsafe scenario asset path: {rel:?}");
        return None;
    }
    lunco_assets_path::relative_path(rel)
}

/// Resolve a manifest asset's relative path to its on-disk cache location under
/// [`scenario_cache_root`], traversal-guarded via [`safe_rel_path`].
fn scenario_asset_path(scenario_id: &[u8; 16], revision: &[u8; 32], rel: &str) -> Option<PathBuf> {
    Some(scenario_cache_root(scenario_id, revision).join(safe_rel_path(rel)?))
}

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

// ── Client: request ──────────────────────────────────────────────────────────

/// Client: on a new scenario manifest, request the assets we don't yet have.
/// Runs unconditionally (registered in `SyncPlugin`) but no-ops on the host and
/// only recomputes when [`RemoteScenarioManifest`] actually changes.
pub fn request_missing_assets(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    probe_state: Res<CacheProbeState>,
    mut downloads: ResMut<AssetDownloads>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if !remote.is_live(*role, connection.0) {
        return;
    }
    let Some(manifest) = remote.manifest.as_ref() else {
        return;
    };
    // The HTTP bytes plane owns the transfer when the host advertises one — see
    // `http_fetch`. Requesting here too would fetch every asset twice.
    if manifest.asset_base_url.is_some() {
        return;
    }
    // Wait for the cache probe to settle for THIS manifest revision before
    // requesting, so assets already in the local cache (marked completed+
    // requested by `drive_cache_probe`) are skipped instead of re-fetched. The
    // `requested` set dedups across frames, so once the probe has settled this
    // loop is a cheap no-op until a new revision lands.
    if probe_state.settled != Some(manifest.revision) {
        return;
    }
    let mut missing = Vec::new();
    for asset in &manifest.assets {
        if downloads.requested.contains(&asset.cid) {
            continue;
        }
        // Not yet requested this session and not a cache-hit → fetch it.
        downloads.requested.insert(asset.cid.clone());
        missing.push(asset.cid.clone());
    }
    if !missing.is_empty() {
        info!(
            "[net] requesting {} missing scenario asset(s)",
            missing.len()
        );
        outbox.0.push((
            SyncChannel::BulkData,
            SyncEnvelope::AssetRequest(AssetRequestMsg { missing }),
        ));
    }
}

// ── Client: reassemble + persist ───────────────────────────────────────────────

/// Client: reassemble queued chunks per CID; on completion verify the content
/// hash and persist to the scenario cache. Fail-closed on hash mismatch.
pub fn reassemble_asset_chunks(
    role: Res<NetworkRole>,
    mut incoming: ResMut<IncomingAssetChunks>,
    mut downloads: ResMut<AssetDownloads>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    persist: Res<AssetPersist>,
    mut rejected: Local<u32>,
) {
    if !remote.is_live(*role, connection.0) || incoming.0.is_empty() {
        return;
    }
    for ch in std::mem::take(&mut incoming.0) {
        // Admission gate at the FIRST chunk of a CID, BEFORE any buffer opens:
        // the CID must be in the current manifest and the sender-controlled
        // `total` must not exceed the manifest's advertised size — otherwise
        // arbitrary CIDs/totals could open unbounded reassembly buffers.
        if !downloads.inflight.contains_key(&ch.cid) {
            let advertised = remote.manifest.as_ref().and_then(|m| {
                m.assets
                    .iter()
                    .find(|a| a.cid.as_slice() == ch.cid.as_slice())
                    .map(|a| a.size)
            });
            let admissible = advertised.is_some_and(|size| ch.total <= size);
            if !admissible {
                // Throttled warn — a hostile/broken sender floods in bursts.
                *rejected = rejected.wrapping_add(1);
                if *rejected % 32 == 1 {
                    warn!(
                        "[net] dropping inadmissible asset chunk (CID not in manifest, or \
                         total {} > advertised {:?}); {} dropped so far",
                        ch.total, advertised, *rejected
                    );
                }
                continue;
            }
            downloads.inflight.insert(
                ch.cid.clone(),
                Inflight {
                    total: ch.total,
                    ..Default::default()
                },
            );
        }
        // Append into the per-CID buffer + feed the running hash (scoped borrow so
        // we can touch `downloads.requested` afterwards without overlapping it).
        let (complete, out_of_order) = {
            let Some(entry) = downloads.inflight.get_mut(&ch.cid) else {
                continue; // unreachable: admitted above
            };
            // `total` was validated + fixed at admission; a later chunk restating
            // a different total (mid-stream inflation) is treated as a bad stream.
            if ch.total != entry.total || ch.offset != entry.buf.len() as u64 {
                (false, true)
            } else {
                entry.buf.extend_from_slice(&ch.data);
                entry.hasher.update(&ch.data);
                (entry.buf.len() as u64 >= entry.total, false)
            }
        };
        if out_of_order {
            warn!("[net] asset chunk out of order (cid); dropping partial download");
            downloads.inflight.remove(&ch.cid);
            downloads.requested.remove(&ch.cid); // allow a future re-request
            continue;
        }
        if !complete {
            continue;
        }
        let Some(done) = downloads.inflight.remove(&ch.cid) else {
            continue;
        };
        // Verify (fail-closed) by comparing the incremental digest to the CID's
        // embedded sha2-256 — no full-buffer re-hash.
        let actual = done.hasher.finalize();
        let expected = cid_from_bytes(&ch.cid).map(|c| c.hash().digest().to_vec());
        if expected.as_deref() != Some(actual.as_slice()) {
            warn!("[net] downloaded asset failed CID verification; discarding");
            downloads.requested.remove(&ch.cid); // retriable on next manifest
            continue;
        }
        // Resolve the cache targets from the manifest and hand the writes off to the
        // async backend (never blocks this system). A CID can appear at SEVERAL paths
        // (two byte-identical files share one content id — the transfer is
        // content-addressed, so the host sends those bytes once). Every path must be
        // materialized, or the scene's `twin://` load misses the duplicate's second path.
        let targets: Vec<StorageHandle> = remote
            .manifest
            .as_ref()
            .map(|m| {
                m.assets
                    .iter()
                    .filter(|a| a.cid.as_slice() == ch.cid.as_slice())
                    .filter_map(|a| asset_storage_handle(&m.scenario_id, &m.revision, &a.path))
                    .collect()
            })
            .unwrap_or_default();
        if targets.is_empty() {
            warn!("[net] verified asset has no manifest entry / safe path; discarding");
            downloads.requested.remove(&ch.cid);
            continue;
        }
        // The CID is complete only once EVERY one of its paths is written, so the
        // outcome drain must see one report per write (see `AssetPersist.pending`).
        downloads
            .pending_writes
            .insert(ch.cid.clone(), targets.len());
        for handle in targets {
            submit_persist(persist.tx.clone(), ch.cid.clone(), handle, done.buf.clone());
        }
    }
}

/// Drain async persist outcomes: a failed write drops the CID from `requested`
/// so a later manifest can re-fetch it; a success is already accounted for.
/// Client-only.
pub fn drain_persist_results(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    persist: Res<AssetPersist>,
    mut downloads: ResMut<AssetDownloads>,
) {
    if !remote.is_live(*role, connection.0) {
        return;
    }
    while let Ok(outcome) = persist.rx.try_recv() {
        if !outcome.ok {
            // Any failed write for this CID fails the whole asset: forget the
            // remaining tally so a straggler success can't mark it complete.
            downloads.pending_writes.remove(&outcome.cid);
            downloads.requested.remove(&outcome.cid); // retriable on next manifest
            continue;
        }
        // Complete only when the last of this CID's writes reports in (a CID may
        // occupy several manifest paths).
        if let Some(remaining) = downloads.pending_writes.get_mut(&outcome.cid) {
            *remaining -= 1;
            if *remaining == 0 {
                downloads.pending_writes.remove(&outcome.cid);
                downloads.completed.insert(outcome.cid);
            }
        }
        // No tally (e.g. a failure already cleared it) → ignore the straggler.
    }
}

// ── Client: cross-session cache-hit probe (G1) ─────────────────────────────────

/// Read the newest valid admission record for one immutable revision.
async fn read_revision_index(
    root: &std::path::Path,
    scenario_id: &[u8; 16],
    revision: &[u8; 32],
    limits: ScenarioCacheLimits,
) -> Option<ScenarioIndex> {
    let records = revision_catalog_records(root, limits).await;
    for (admission, handle) in records.into_iter().rev().take(limits.max_admission_records) {
        let Some(bytes) = storage_read(&handle, limits.max_record_bytes).await else {
            continue;
        };
        let index = match serde_json::from_slice::<ScenarioIndex>(&bytes) {
            Ok(index) => index,
            Err(error) => {
                warn!("[net] invalid scenario catalog record: {error}");
                continue;
            }
        };
        if index.summary.scenario_id != *scenario_id
            || index.summary.revision != *revision
            || index.summary.cached_at_unix_ns != admission.cached_at_unix_ns
            || index.summary.admission_token != admission.token
        {
            warn!("[net] scenario catalog identity does not match its admitted path");
            continue;
        }
        return Some(index);
    }
    None
}

fn parse_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut bytes = [0; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] =
            ((pair[0] as char).to_digit(16)? * 16 + (pair[1] as char).to_digit(16)?) as u8;
    }
    Some(bytes)
}

fn record_admission(handle: &StorageHandle) -> Option<CacheAdmission> {
    let name = handle.as_file_path()?.file_name()?.to_str()?;
    let value = name.strip_prefix(".scenario-")?.strip_suffix(".json")?;
    let (timestamp, token) = value.split_once('-')?;
    if timestamp.len() != 20 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    parse_hex::<16>(token)?;
    Some(CacheAdmission {
        cached_at_unix_ns: timestamp.parse().ok()?,
        token: token.to_string(),
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_directory(
    handle: &StorageHandle,
) -> lunco_storage::StorageResult<Vec<StorageHandle>> {
    use lunco_storage::Storage;
    lunco_storage::FileStorage::new()
        .read_directory(handle)
        .await
}

#[cfg(target_arch = "wasm32")]
async fn storage_directory(
    handle: &StorageHandle,
) -> lunco_storage::StorageResult<Vec<StorageHandle>> {
    lunco_storage::OpfsStorage::new()
        .read_directory(handle)
        .await
}

async fn catalog_directory(
    handle: &StorageHandle,
    limits: ScenarioCacheLimits,
) -> Vec<StorageHandle> {
    if limits.max_namespace_entries == 0
        || limits.max_admission_records == 0
        || limits.max_record_bytes == 0
    {
        warn!("[net] scenario cache catalog budgets must be non-zero");
        return Vec::new();
    }
    match storage_directory(handle).await {
        Ok(entries) if entries.len() <= limits.max_namespace_entries => entries,
        Ok(_) => {
            warn!(
                "[net] scenario catalog namespace exceeds entry budget: {}",
                handle.display_name()
            );
            Vec::new()
        }
        Err(lunco_storage::StorageError::NotFound) => Vec::new(),
        Err(error) => {
            warn!("[net] scenario catalog directory read failed: {error}");
            Vec::new()
        }
    }
}

async fn revision_catalog_records(
    root: &std::path::Path,
    limits: ScenarioCacheLimits,
) -> Vec<(CacheAdmission, StorageHandle)> {
    let mut records: Vec<_> = catalog_directory(&StorageHandle::File(root.to_path_buf()), limits)
        .await
        .into_iter()
        .filter_map(|handle| record_admission(&handle).map(|admission| (admission, handle)))
        .collect();
    records.sort_by(|a, b| {
        (a.0.cached_at_unix_ns, &a.0.token).cmp(&(b.0.cached_at_unix_ns, &b.0.token))
    });
    records
}

async fn read_cache_catalog(limits: ScenarioCacheLimits) -> Vec<CachedTwinSummary> {
    let mut registry = CachedTwinsRegistry::default();
    let scenarios = catalog_directory(
        &StorageHandle::File(lunco_assets_core::scenarios_dir()),
        limits,
    )
    .await;
    let mut remaining_revisions = limits.max_namespace_entries;
    for scenario in scenarios {
        let Some(id) = scenario
            .as_file_path()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .and_then(parse_hex::<16>)
        else {
            continue;
        };
        for revision in catalog_directory(&scenario, limits).await {
            let Some(revision) = revision
                .as_file_path()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .and_then(parse_hex::<32>)
            else {
                continue;
            };
            if remaining_revisions == 0 {
                warn!("[net] scenario catalog scan exhausted revision budget");
                return registry.entries;
            }
            remaining_revisions -= 1;
            if let Some(index) =
                read_revision_index(&scenario_cache_root(&id, &revision), &id, &revision, limits)
                    .await
            {
                registry.admit(index.summary);
            }
        }
    }
    registry.entries
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_delete(handle: &StorageHandle) -> lunco_storage::StorageResult<()> {
    use lunco_storage::Storage;
    lunco_storage::FileStorage::new().delete(handle).await
}

#[cfg(target_arch = "wasm32")]
async fn storage_delete(handle: &StorageHandle) -> lunco_storage::StorageResult<()> {
    lunco_storage::OpfsStorage::new().delete(handle).await
}

async fn retain_catalog_records(root: &std::path::Path, limits: ScenarioCacheLimits) {
    let records = revision_catalog_records(root, limits).await;
    let remove = records.len().saturating_sub(limits.max_admission_records);
    for (_, handle) in records.into_iter().take(remove) {
        if let Err(error) = storage_delete(&handle).await {
            if !matches!(error, lunco_storage::StorageError::NotFound) {
                warn!("[net] scenario catalog retention failed: {error}");
            }
        }
    }
}

async fn persist_catalog_record(
    root: &std::path::Path,
    admission: &CacheAdmission,
    bytes: Vec<u8>,
    limits: ScenarioCacheLimits,
) {
    let handle = StorageHandle::File(root.join(catalog_record_name(admission)));
    if do_write(handle, bytes).await {
        retain_catalog_records(root, limits).await;
    }
}

/// True iff the cache file for an asset is present on disk/OPFS.
async fn cached_asset_exists(path: &std::path::Path) -> bool {
    storage_exists(&StorageHandle::File(path.to_path_buf())).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_read(handle: &StorageHandle, max_bytes: usize) -> Option<Vec<u8>> {
    match lunco_storage::FileStorage::new()
        .read_bounded(handle, max_bytes)
        .await
    {
        Ok(bytes) => Some(bytes),
        Err(lunco_storage::StorageError::NotFound) => None,
        Err(error) => {
            warn!("[net] scenario cache read failed: {error}");
            None
        }
    }
}
#[cfg(target_arch = "wasm32")]
async fn storage_read(handle: &StorageHandle, max_bytes: usize) -> Option<Vec<u8>> {
    match lunco_storage::OpfsStorage::new()
        .read_bounded(handle, max_bytes)
        .await
    {
        Ok(bytes) => Some(bytes),
        Err(lunco_storage::StorageError::NotFound) => None,
        Err(error) => {
            warn!("[net] scenario cache read failed: {error}");
            None
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_exists(handle: &StorageHandle) -> bool {
    // `FileStorage` exposes no `exists` on the trait; a direct stat is cheapest.
    matches!(handle, StorageHandle::File(p) if p.exists())
}
#[cfg(target_arch = "wasm32")]
async fn storage_exists(handle: &StorageHandle) -> bool {
    lunco_storage::OpfsStorage::new().exists(handle).await
}

/// Async probe body: for each manifest asset, mark it cached iff the index records
/// the same CID at that path AND the file is present. Returns the cached CID set
/// (possibly empty) for `revision` — always sent, so [`CacheProbeState::settled`]
/// always advances and [`request_missing_assets`] never stalls waiting on a probe.
async fn run_cache_probe(
    scenario_id: [u8; 16],
    revision: [u8; 32],
    assets: Vec<(Vec<u8>, String)>,
    limits: ScenarioCacheLimits,
) -> ProbeOutcome {
    let by_path: HashMap<String, Vec<u8>> = read_revision_index(
        &scenario_cache_root(&scenario_id, &revision),
        &scenario_id,
        &revision,
        limits,
    )
    .await
    .map(|idx| idx.assets.into_iter().map(|a| (a.path, a.cid)).collect())
    .unwrap_or_default();
    let mut cached = HashSet::new();
    for (cid, rel) in &assets {
        if by_path.get(rel).is_some_and(|c| c == cid) {
            if let Some(p) = scenario_asset_path(&scenario_id, &revision, rel) {
                if cached_asset_exists(&p).await {
                    cached.insert(cid.clone());
                }
            }
        }
    }
    ProbeOutcome { revision, cached }
}

/// Client: kick a cache probe when a new manifest revision lands, then apply its
/// results — already-cached CIDs go straight into `completed`+`requested` so they
/// are neither re-requested nor block [`AssetDownloads::all_cached`]. Registered
/// **before** [`request_missing_assets`] so `settled` is current when it runs.
pub fn drive_cache_probe(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    probe: Res<AssetCacheProbe>,
    limits: Res<ScenarioCacheLimits>,
    mut state: ResMut<CacheProbeState>,
    mut downloads: ResMut<AssetDownloads>,
) {
    if !remote.is_live(*role, connection.0) {
        return;
    }
    if let Some(m) = remote.manifest.as_ref() {
        if state.kicked != Some(m.revision) {
            state.kicked = Some(m.revision);
            let scenario_id = m.scenario_id;
            let revision = m.revision;
            let limits = *limits;
            let assets: Vec<(Vec<u8>, String)> = m
                .assets
                .iter()
                .map(|a| (a.cid.clone(), a.path.clone()))
                .collect();
            let tx = probe.tx.clone();
            let fut = async move {
                let outcome = run_cache_probe(scenario_id, revision, assets, limits).await;
                let _ = tx.send(outcome);
            };
            #[cfg(not(target_arch = "wasm32"))]
            AsyncComputeTaskPool::get().spawn(fut).detach();
            #[cfg(target_arch = "wasm32")]
            wasm_bindgen_futures::spawn_local(fut);
        }
    }
    while let Ok(outcome) = probe.rx.try_recv() {
        for cid in &outcome.cached {
            downloads.completed.insert(cid.clone());
            downloads.requested.insert(cid.clone());
        }
        state.settled = Some(outcome.revision);
    }
}

/// Client: once a scenario is fully cached, persist its immutable admission so
/// a later session's probe recognizes it and the cached-twin menu can list it.
/// Fires once per admitted manifest. Client-only.
pub fn write_scenario_index(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    downloads: Res<AssetDownloads>,
    limits: Res<ScenarioCacheLimits>,
    mut registry: ResMut<CachedTwinsRegistry>,
    mut written: Local<Option<String>>,
) {
    if !remote.is_live(*role, connection.0) {
        return;
    }
    let (Some(m), Some(admission)) = (remote.manifest.as_ref(), remote.cache_admission.as_ref())
    else {
        warn_once!("[net] live scenario has no cache catalog admission");
        return;
    };
    if written.as_ref() == Some(&admission.token) || !downloads.all_cached(m) {
        return;
    }
    if limits.max_admission_records == 0
        || limits.max_namespace_entries == 0
        || limits.max_record_bytes == 0
    {
        warn!("[net] scenario cache catalog budgets must be non-zero");
        return;
    }
    *written = Some(admission.token.clone());
    let summary = CachedTwinSummary {
        scenario_id: m.scenario_id,
        name: m.name.clone(),
        default_scene: m.default_scene.clone(),
        total_bytes: m.assets.iter().map(|a| a.size).sum(),
        revision: m.revision,
        cached_at_unix_ns: admission.cached_at_unix_ns,
        admission_token: admission.token.clone(),
    };
    let index = ScenarioIndex {
        summary: summary.clone(),
        assets: m
            .assets
            .iter()
            .map(|a| ScenarioIndexAsset {
                path: a.path.clone(),
                cid: a.cid.clone(),
                size: a.size,
            })
            .collect(),
    };
    let bytes = match serde_json::to_vec(&index) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("[net] scenario catalog serialization failed: {error}");
            return;
        }
    };
    if bytes.len() > limits.max_record_bytes {
        warn!("[net] scenario catalog record exceeds metadata budget");
        return;
    }
    registry.admit(summary);
    let root = scenario_cache_root(&m.scenario_id, &m.revision);
    let admission = admission.clone();
    let limits = *limits;
    let fut = async move {
        persist_catalog_record(&root, &admission, bytes, limits).await;
    };
    #[cfg(not(target_arch = "wasm32"))]
    AsyncComputeTaskPool::get().spawn(fut).detach();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(fut);
}

/// Read retained revision records asynchronously at boot, merging by pinned
/// admission order even if this session downloaded a Twin before the scan ends.
pub fn refresh_cached_twins_registry(
    index: Res<CachedTwinsIndex>,
    limits: Res<ScenarioCacheLimits>,
    mut registry: ResMut<CachedTwinsRegistry>,
    mut kicked: Local<bool>,
) {
    if !*kicked {
        *kicked = true;
        let tx = index.tx.clone();
        let limits = *limits;
        let fut = async move {
            let _ = tx.send(read_cache_catalog(limits).await);
        };
        #[cfg(not(target_arch = "wasm32"))]
        AsyncComputeTaskPool::get().spawn(fut).detach();
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(fut);
    }
    if let Ok(entries) = index.rx.try_recv() {
        for summary in entries {
            registry.admit(summary);
        }
    }
}

/// G2: project [`AssetDownloads`] + the remote manifest into
/// [`ScenarioDownloadStatus`] for the download-progress overlay. Completed assets
/// count their full size; an in-flight asset counts its buffered bytes so far.
/// Client-only.
pub fn update_scenario_download_status(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    downloads: Res<AssetDownloads>,
    mut status: ResMut<ScenarioDownloadStatus>,
) {
    if !remote.is_live(*role, connection.0) {
        return;
    }
    let Some(m) = remote.manifest.as_ref() else {
        *status = ScenarioDownloadStatus::default();
        return;
    };
    if m.assets.is_empty() {
        *status = ScenarioDownloadStatus::default();
        return;
    }
    let total: u64 = m.assets.iter().map(|a| a.size).sum();
    let mut bytes_done: u64 = 0;
    let mut assets_done = 0usize;
    for a in &m.assets {
        if downloads.completed.contains(&a.cid) {
            bytes_done += a.size;
            assets_done += 1;
        } else if let Some(inf) = downloads.inflight.get(&a.cid) {
            bytes_done += inf.buf.len() as u64;
        }
    }
    let all_cached = downloads.all_cached(m);
    *status = ScenarioDownloadStatus {
        active: !all_cached,
        name: m.name.clone(),
        assets_done,
        assets_total: m.assets.len(),
        bytes_done: bytes_done.min(total),
        bytes_total: total,
    };
}

/// Exact downloaded mount admitted by UUID and revision, with its real asset
/// authority and root. Ownership is acquired only when this admission creates
/// its authority; an already mounted cache remains owned by its existing caller.
#[derive(Debug)]
pub struct ScenarioTwinMount {
    pub path: String,
    pub authority: String,
    pub root: PathBuf,
    pub owns_mount: bool,
}

/// Mount the exact verified UUID/revision cache and return its typed ownership.
/// A live logical name bound to another root is a conflict; a name alone never
/// proves that an editable checkout contains this scenario's bytes.
pub fn mount_scenario_twin(
    twins: &lunco_assets_core::twin_source::TwinRoots,
    scenario_id: &[u8; 16],
    revision: &[u8; 32],
    name: &str,
    rel: &str,
) -> Result<ScenarioTwinMount, lunco_assets_core::twin_source::TwinRootsError> {
    mount_cached_root(twins, scenario_cache_root(scenario_id, revision), name, rel)
}

fn mount_cached_root(
    twins: &lunco_assets_core::twin_source::TwinRoots,
    root: PathBuf,
    name: &str,
    rel: &str,
) -> Result<ScenarioTwinMount, lunco_assets_core::twin_source::TwinRootsError> {
    use lunco_assets_core::twin_source::TwinRootsError;
    safe_rel_path(rel).ok_or_else(|| {
        TwinRootsError::AssetResolution(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe scenario scene path `{rel}`"),
        )
    })?;
    let existing = twins.name_for_root(&root)?;
    let existing_root = match existing.as_ref() {
        Some(authority) => Some(
            twins
                .root_for(authority)?
                .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.clone()))?,
        ),
        None => None,
    };
    let existing_logical = twins.mounted_name_for_logical(name)?;
    if let Some(logical_mount) = existing_logical.as_ref() {
        let logical_root = twins
            .root_for(logical_mount)?
            .ok_or_else(|| TwinRootsError::UnknownAuthority(logical_mount.clone()))?;
        if existing_root.as_ref() != Some(&logical_root) {
            return Err(TwinRootsError::AssetResolution(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "logical Twin `{name}` is already mounted from another root; downloaded scenario requires its exact UUID/revision cache"
                ),
            ));
        }
    }
    let assigned = twins.register(name, root)?;
    let owns_mount = existing_logical.as_deref() != Some(assigned.as_str());
    let logical = twins.logical_name(&assigned)?;
    if logical != name {
        if owns_mount {
            twins.unregister_name(&assigned)?;
        }
        return Err(TwinRootsError::LogicalIdentityMismatch {
            requested: name.to_string(),
            assigned: logical,
        });
    }
    let root = twins
        .root_for(&assigned)?
        .ok_or_else(|| TwinRootsError::UnknownAuthority(assigned.clone()))?;
    Ok(ScenarioTwinMount {
        path: lunco_assets_core::twin_uri(&assigned, rel),
        authority: assigned,
        root,
        owns_mount,
    })
}

/// The storage handle for a scenario asset's cache location. A
/// [`StorageHandle::File`] on **both** platforms (native: absolute, under
/// `cache_dir()`; web: the same path fed to `OpfsStorage`, which maps its
/// components onto the OPFS tree) — so only the backend, not the handle, differs.
pub(crate) fn asset_storage_handle(
    scenario_id: &[u8; 16],
    revision: &[u8; 32],
    rel: &str,
) -> Option<StorageHandle> {
    Some(StorageHandle::File(scenario_asset_path(
        scenario_id,
        revision,
        rel,
    )?))
}

/// Spawn the verify-passed asset's write on the platform's async executor and
/// report the outcome back over `tx`. The write NEVER runs on the calling
/// system: native → `AsyncComputeTaskPool` (real thread); web → `spawn_local`
/// (async OPFS on the main thread, non-blocking). The awaited body is the only
/// native/web divergence — see [`do_write`].
pub(crate) fn submit_persist(
    tx: Sender<PersistOutcome>,
    cid: Vec<u8>,
    handle: StorageHandle,
    bytes: Vec<u8>,
) {
    let fut = async move {
        let ok = do_write(handle, bytes).await;
        let _ = tx.send(PersistOutcome { cid, ok });
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        AsyncComputeTaskPool::get().spawn(fut).detach();
    }
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(fut);
    }
}

/// Write reassembled+verified asset bytes to the scenario cache. The ONLY
/// native/web-divergent code in the client path: native uses the `Send`
/// [`lunco_storage::Storage`] trait over `FileStorage`; web uses
/// [`lunco_storage::OpfsStorage`]'s inherent (non-`Send`) async methods.
#[cfg(not(target_arch = "wasm32"))]
async fn do_write(handle: StorageHandle, bytes: Vec<u8>) -> bool {
    use lunco_storage::Storage;
    match lunco_storage::FileStorage::new()
        .write(&handle, &bytes)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!("[net] asset cache write failed: {e}");
            false
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn do_write(handle: StorageHandle, bytes: Vec<u8>) -> bool {
    match lunco_storage::OpfsStorage::new()
        .write(&handle, &bytes)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!("[net] asset cache write failed: {e}");
            false
        }
    }
}

// ── Host: serve ────────────────────────────────────────────────────────────────

/// Host: turn queued asset requests into off-thread read+chunk jobs. The main
/// thread only does cheap CID→path lookups; the whole-file reads + slicing run on
/// the `AsyncComputeTaskPool` so a large-asset request never stalls the ferry.
pub fn serve_asset_requests(
    role: Res<NetworkRole>,
    mut pending: ResMut<PendingAssetRequests>,
    paths: Res<HostAssetPaths>,
    mut tasks: ResMut<AssetServeTasks>,
) {
    if !role.is_host() || pending.0.is_empty() {
        return;
    }
    let pool = AsyncComputeTaskPool::get();
    for (session, cids) in pending.0.drain(..) {
        let jobs: Vec<(Vec<u8>, PathBuf)> = cids
            .into_iter()
            .filter_map(|cid| match paths.0.get(&cid) {
                Some(p) => Some((cid, p.clone())),
                None => {
                    warn!("[net] asset request for a CID not in the current scenario; ignoring");
                    None
                }
            })
            .collect();
        if jobs.is_empty() {
            continue;
        }
        info!(
            "[net] serving {} scenario asset(s) to session {:?}",
            jobs.len(),
            session
        );
        tasks
            .0
            .push((session, pool.spawn(async move { read_and_chunk(jobs) })));
    }
}

/// Off-thread body of [`serve_asset_requests`]: read each requested file (through
/// the storage API) and slice it into ordered [`AssetChunkMsg`]s. A file that
/// can't be read is skipped (logged) — the client simply never completes it and
/// can re-request on the next manifest.
fn read_and_chunk(jobs: Vec<(Vec<u8>, PathBuf)>) -> Vec<AssetChunkMsg> {
    let mut out = Vec::new();
    for (cid, path) in jobs {
        let bytes = match lunco_storage::read_file_sync(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!("[net] asset serve: read {path:?} failed: {e}");
                continue;
            }
        };
        let total = bytes.len() as u64;
        if total == 0 {
            // Empty file: one empty chunk so the client can complete it.
            out.push(AssetChunkMsg {
                cid: cid.clone(),
                offset: 0,
                total: 0,
                data: Vec::new(),
            });
            continue;
        }
        let mut offset = 0u64;
        for chunk in bytes.chunks(ASSET_CHUNK_SIZE) {
            out.push(AssetChunkMsg {
                cid: cid.clone(),
                offset,
                total,
                data: chunk.to_vec(),
            });
            offset += chunk.len() as u64;
        }
    }
    out
}

// ── Promote: downloaded (read-only) scenario → editable on-disk Twin ──────────

/// Command: materialize the currently-loaded downloaded scenario into an
/// **editable** on-disk Twin at `folder`, add it to the workspace, and swap the
/// running scene to it. The counterpart to the default read-only consume — "keep
/// & edit this scenario". Empty `folder` = a GUI should present a folder picker
/// first. Native-only in effect: web has no ambient folder filesystem (File
/// System Access is a TODO); the wasm path logs and no-ops.
///
/// Local action (not networked) — it promotes *this* peer's local download.
#[lunco_core::Command(default)]
pub struct PromoteScenario {
    /// Target folder that becomes the new Twin's root.
    pub folder: String,
}

#[lunco_core::on_command(PromoteScenario)]
fn on_promote_scenario(
    trigger: On<PromoteScenario>,
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    connection: Res<lunco_core_session::ClientConnection>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut commands: Commands,
) {
    if !remote.is_live(*role, connection.0) {
        warn!("[promote] no live scenario connection");
        return;
    }
    let folder = trigger.event().folder.clone();
    if folder.is_empty() {
        warn!("[promote] no target folder given (a GUI should present a folder picker first)");
        return;
    }
    let Some(manifest) = remote.manifest.clone() else {
        warn!("[promote] no downloaded scenario to promote");
        return;
    };
    promote_scenario_to_folder(&manifest, &folder, &mut workspace, &mut commands);
}

/// Materialize + promote (native). Copies each manifest asset from the scenario
/// cache into `folder` through the storage API (no raw dir walk — only the
/// scenario's own assets), writes a `twin.toml` that **preserves the scenario
/// identity as the Twin uuid** (so a future re-download / bidirectional sync
/// recognizes it), then `add_twin` + `TwinAdded` — which the USD observer turns
/// into a `twin://` scene load backed by the promoted folder instead of the
/// read-only revision cache. The workspace asset owner assigns the promoted
/// Twin a live load authority; stable logical provenance remains owner-derived.
#[cfg(not(target_arch = "wasm32"))]
fn promote_scenario_to_folder(
    manifest: &ScenarioManifestMsg,
    folder: &str,
    workspace: &mut lunco_workspace::WorkspaceResource,
    commands: &mut Commands,
) {
    let target = PathBuf::from(folder);
    let cache_root = scenario_cache_root(&manifest.scenario_id, &manifest.revision);
    for asset in &manifest.assets {
        let Some(rel) = safe_rel_path(&asset.path) else {
            error!("[promote] unsafe asset path {:?}; aborting", asset.path);
            return;
        };
        let src = cache_root.join(&rel);
        let dst = target.join(&rel);
        let bytes = match lunco_storage::read_file_sync(&src) {
            Ok(b) => b,
            Err(e) => {
                error!("[promote] read cached asset {src:?}: {e}");
                return;
            }
        };
        if let Err(e) = lunco_storage::write_file_sync(&dst, &bytes) {
            error!("[promote] write {dst:?}: {e}");
            return;
        }
    }

    let mut tm = lunco_twin::TwinManifest::new(manifest.name.clone());
    tm.uuid = Some(uuid::Uuid::from_bytes(manifest.scenario_id));
    tm.usd = Some(lunco_twin::UsdManifest {
        default_scene: manifest.default_scene.clone(),
        // A downloaded scenario's layout is the host's, and the wire manifest
        // does not carry it — so leave it undeclared and let the conventional
        // fallback apply, rather than assert a layout we were not told.
        scenes: None,
    });

    let mut twin = match lunco_twin::TwinMode::open(&target) {
        Ok(lunco_twin::TwinMode::Folder(t)) | Ok(lunco_twin::TwinMode::Twin(t)) => t,
        Ok(lunco_twin::TwinMode::Orphan(_)) => {
            error!("[promote] {target:?} is a file, not a folder");
            return;
        }
        Err(e) => {
            error!("[promote] open {target:?}: {e}");
            return;
        }
    };
    if let Err(e) = twin.promote_to_twin(tm) {
        error!("[promote] write twin.toml in {target:?}: {e}");
        return;
    }
    let id = workspace.add_twin(twin);
    commands.trigger(lunco_workspace::TwinAdded { twin: id });
    info!(
        "[promote] scenario '{}' promoted to editable Twin at {target:?}",
        manifest.name
    );
}

#[cfg(target_arch = "wasm32")]
fn promote_scenario_to_folder(
    _manifest: &ScenarioManifestMsg,
    _folder: &str,
    _workspace: &mut lunco_workspace::WorkspaceResource,
    _commands: &mut Commands,
) {
    warn!(
        "[promote] promotion needs a native filesystem folder; web (File System Access) is a TODO"
    );
}

lunco_core::register_commands!(on_promote_scenario);

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_networking_scenario::cid_for_content;

    /// Two byte-identical files share ONE CID (the transfer is content-addressed,
    /// so the host streams those bytes once). The client must materialize the blob
    /// at EVERY manifest path that carries the CID — resolving with `.find()` wrote
    /// only the first, and the scene's `twin://` load then 404'd on the duplicate's path.
    #[test]
    fn duplicate_cid_resolves_to_every_manifest_path() {
        let id = [3u8; 16];
        let shared = cid_for_content(b"same bytes").to_bytes();
        let other = cid_for_content(b"different").to_bytes();
        let assets = [
            ("rover.glb", shared.clone()),
            ("structures/rover.glb", shared.clone()),
            ("scene.usda", other),
        ];

        // Mirrors `reassemble_asset_chunks`'s target resolution for a completed CID.
        let targets: Vec<_> = assets
            .iter()
            .filter(|(_, cid)| cid.as_slice() == shared.as_slice())
            .filter_map(|(path, _)| asset_storage_handle(&id, &[4; 32], path))
            .collect();

        assert_eq!(
            targets.len(),
            2,
            "both paths sharing the CID must be written"
        );
        let expect =
            |rel: &str| StorageHandle::File(scenario_asset_path(&id, &[4; 32], rel).unwrap());
        assert!(targets.contains(&expect("rover.glb")));
        assert!(targets.contains(&expect("structures/rover.glb")));
    }

    #[test]
    fn catalog_retention_and_selection_use_admission_order() {
        bevy::tasks::block_on(async {
            let root = tempfile::tempdir().unwrap();
            let id = [1; 16];
            let revision = [2; 32];
            let limits = ScenarioCacheLimits {
                max_admission_records: 2,
                max_record_bytes: 1024,
                ..Default::default()
            };
            // Finish the oldest metadata write last, including a same-revision rename.
            for timestamp in [3, 2, 1] {
                let admission = CacheAdmission {
                    cached_at_unix_ns: timestamp,
                    token: format!("{timestamp:032x}"),
                };
                let index = ScenarioIndex {
                    summary: CachedTwinSummary {
                        scenario_id: id,
                        revision,
                        name: format!("admission {timestamp}"),
                        default_scene: Some(format!("scene-{timestamp}.usda")),
                        total_bytes: 0,
                        cached_at_unix_ns: timestamp,
                        admission_token: admission.token.clone(),
                    },
                    assets: Vec::new(),
                };
                persist_catalog_record(
                    root.path(),
                    &admission,
                    serde_json::to_vec(&index).unwrap(),
                    limits,
                )
                .await;
            }
            let records = revision_catalog_records(root.path(), limits).await;
            assert_eq!(records.len(), 2);
            assert_eq!(records[0].0.cached_at_unix_ns, 2);
            let selected = read_revision_index(root.path(), &id, &revision, limits)
                .await
                .unwrap();
            assert_eq!(selected.summary.name, "admission 3");
            assert_eq!(
                selected.summary.default_scene.as_deref(),
                Some("scene-3.usda")
            );
            let expanded_limits = ScenarioCacheLimits {
                max_admission_records: 3,
                ..limits
            };
            for (timestamp, bytes) in [
                (4, vec![b'x'; limits.max_record_bytes + 1]),
                (5, b"{".to_vec()),
            ] {
                persist_catalog_record(
                    root.path(),
                    &CacheAdmission {
                        cached_at_unix_ns: timestamp,
                        token: format!("{timestamp:032x}"),
                    },
                    bytes,
                    expanded_limits,
                )
                .await;
            }
            let recovered = read_revision_index(root.path(), &id, &revision, expanded_limits)
                .await
                .unwrap();
            assert_eq!(recovered.summary, selected.summary);
            assert!(
                read_revision_index(
                    root.path(),
                    &id,
                    &revision,
                    ScenarioCacheLimits {
                        max_record_bytes: 0,
                        ..expanded_limits
                    }
                )
                .await
                .is_none()
            );
            assert!(
                read_revision_index(root.path(), &[4; 16], &revision, limits)
                    .await
                    .is_none()
            );
            let mut registry = CachedTwinsRegistry::default();
            registry.admit(selected.summary.clone());
            let mut late = selected.summary;
            late.cached_at_unix_ns = 1;
            late.name = "late old metadata".into();
            registry.admit(late);
            assert_eq!(registry.entries[0].name, "admission 3");
        });
    }

    #[test]
    fn catalog_record_paths_validate_canonical_identities() {
        let admission = CacheAdmission {
            cached_at_unix_ns: 42,
            token: "ab".repeat(16),
        };
        assert_eq!(
            record_admission(&StorageHandle::File(catalog_record_name(&admission).into())),
            Some(admission)
        );
        assert!(
            record_admission(&StorageHandle::File(
                ".scenario-00000000000000000042-../escape.json".into()
            ))
            .is_none()
        );
        assert!(parse_hex::<16>(&"ff".repeat(16)).is_some());
        assert!(parse_hex::<16>(&"FF".repeat(16)).is_none());
        assert!(parse_hex::<32>("../escape").is_none());
    }

    #[test]
    fn downloaded_mount_requires_exact_root_and_stable_identity() {
        let first = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let roots = lunco_assets_core::TwinRoots::default();
        let a =
            mount_cached_root(&roots, first.path().to_path_buf(), "shared", "scene.usda").unwrap();
        let repeated =
            mount_cached_root(&roots, first.path().to_path_buf(), "shared", "scene.usda").unwrap();
        assert_eq!(a.authority, repeated.authority);
        assert_eq!(a.root, repeated.root);
        assert!(a.owns_mount);
        assert!(!repeated.owns_mount);
        let aliases = lunco_assets_core::TwinRoots::default();
        aliases.register("editor", first.path()).unwrap();
        let shared = aliases.register("shared", first.path()).unwrap();
        let borrowed =
            mount_cached_root(&aliases, first.path().to_path_buf(), "shared", "scene.usda")
                .unwrap();
        assert_eq!(borrowed.authority, shared);
        assert!(!borrowed.owns_mount);
        assert!(matches!(
            mount_cached_root(&roots, other.path().to_path_buf(), "shared", "scene.usda"),
            Err(lunco_assets_core::TwinRootsError::AssetResolution(
                std::io::ErrorKind::AlreadyExists,
                _
            ))
        ));
        assert_eq!(roots.names().unwrap(), vec![a.authority.clone()]);
        assert!(
            mount_cached_root(
                &roots,
                first.path().to_path_buf(),
                "shared",
                "../scene.usda"
            )
            .is_err()
        );
        roots.unregister_name(&a.authority).unwrap();
        let replacement =
            mount_cached_root(&roots, other.path().to_path_buf(), "shared", "scene.usda").unwrap();
        assert_ne!(replacement.authority, a.authority);
        assert_eq!(
            roots.logical_name(&replacement.authority).unwrap(),
            "shared"
        );
    }

    #[test]
    fn cache_paths_isolate_uuid_and_revision() {
        let id = [7; 16];
        let old_revision = [1; 32];
        let next_revision = [2; 32];
        let old = scenario_asset_path(&id, &old_revision, "models/vehicle.bin").unwrap();
        let next = scenario_asset_path(&id, &next_revision, "models/vehicle.bin").unwrap();
        assert_ne!(old, next);
        assert!(old.starts_with(scenario_cache_root(&id, &old_revision)));
        assert!(next.starts_with(scenario_cache_root(&id, &next_revision)));
        assert_ne!(
            scenario_cache_root(&id, &next_revision),
            scenario_cache_root(&[8; 16], &next_revision)
        );
        let admission = CacheAdmission {
            cached_at_unix_ns: 10,
            token: "ab".repeat(16),
        };
        assert_ne!(
            scenario_cache_root(&id, &old_revision).join(catalog_record_name(&admission)),
            scenario_cache_root(&id, &next_revision).join(catalog_record_name(&admission))
        );
        assert_eq!(hex_bytes(&[0, 0xff]), "00ff");
    }

    #[test]
    fn asset_path_rejects_traversal() {
        let id = [7u8; 16];
        assert!(scenario_asset_path(&id, &[4; 32], "scenes/main.usda").is_some());
        assert!(scenario_asset_path(&id, &[4; 32], "../escape").is_none());
        assert!(scenario_asset_path(&id, &[4; 32], "a/../../b").is_none());
        assert!(scenario_asset_path(&id, &[4; 32], "a//b").is_none()); // empty segment
    }

    #[test]
    fn read_and_chunk_slices_and_preserves_offsets() {
        // A file bigger than one chunk → multiple ordered chunks, contiguous offsets.
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp = tmp_dir.path().join("asset.bin");
        let data = vec![0xABu8; ASSET_CHUNK_SIZE + 123];
        lunco_storage::write_file_sync(&tmp, &data).unwrap();
        let cid = cid_for_content(&data).to_bytes();
        let chunks = read_and_chunk(vec![(cid.clone(), tmp)]);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[0].data.len(), ASSET_CHUNK_SIZE);
        assert_eq!(chunks[1].offset, ASSET_CHUNK_SIZE as u64);
        assert_eq!(chunks[1].data.len(), 123);
        assert!(chunks.iter().all(|c| c.total == data.len() as u64));
        // Reassembled + verified round-trips to the same bytes.
        let mut buf = Vec::new();
        for c in &chunks {
            buf.extend_from_slice(&c.data);
        }
        assert_eq!(buf, data);
        assert_eq!(cid_for_content(&buf).to_bytes(), cid);
    }

    #[test]
    fn incremental_hash_matches_cid_digest() {
        // Mirrors the client verify path: feed chunks to a running Sha256, then
        // compare finalize() to the CID's embedded sha2-256 digest (no re-hash).
        let data = vec![0x5Au8; ASSET_CHUNK_SIZE * 2 + 7];
        let cid = cid_for_content(&data).to_bytes();
        let mut hasher = Sha256::new();
        for chunk in data.chunks(ASSET_CHUNK_SIZE) {
            hasher.update(chunk);
        }
        let actual = hasher.finalize();
        let expected = cid_from_bytes(&cid).map(|c| c.hash().digest().to_vec());
        assert_eq!(expected.as_deref(), Some(actual.as_slice()));
        // A single-byte change must fail the same comparison.
        let mut tampered = data.clone();
        tampered[0] ^= 0xFF;
        let mut h2 = Sha256::new();
        h2.update(&tampered);
        assert_ne!(Some(h2.finalize().as_slice()), expected.as_deref());
    }
}

/// Reliable document/ownership traffic waiting for its exact scene admission.
/// Byte admission uses the transport's hard envelope budget.
#[derive(Resource, Default)]
pub(crate) struct DeferredSceneMessages {
    entries: Vec<DeferredSceneMessage>,
    bytes: usize,
    attempted_scope: Option<lunco_core_session::ReplicationScope>,
}

struct DeferredSceneMessage {
    sender: SessionId,
    envelope: SyncEnvelope,
    bytes: usize,
}

impl DeferredSceneMessages {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn needs_retry(&self, scope: Option<lunco_core_session::ReplicationScope>) -> bool {
        !self.is_empty() && scope.is_some() && scope != self.attempted_scope
    }

    pub(crate) fn take(
        &mut self,
        scope: Option<lunco_core_session::ReplicationScope>,
    ) -> Vec<(SessionId, SyncEnvelope)> {
        self.bytes = 0;
        self.attempted_scope = scope;
        std::mem::take(&mut self.entries)
            .into_iter()
            .map(|entry| (entry.sender, entry.envelope))
            .collect()
    }

    pub(crate) fn admit(
        &mut self,
        sender: SessionId,
        envelope: SyncEnvelope,
    ) -> Result<(), String> {
        let bytes = crate::codec::serialize_env(&envelope)
            .ok_or_else(|| {
                "cannot encode deferred scene message within the envelope budget".to_owned()
            })?
            .len();
        let total = self
            .bytes
            .checked_add(bytes)
            .filter(|total| *total <= crate::codec::MAX_ENVELOPE_BYTES)
            .ok_or_else(|| "deferred scene replay exceeds envelope budget".to_owned())?;
        self.entries.push(DeferredSceneMessage {
            sender,
            envelope,
            bytes,
        });
        self.bytes = total;
        Ok(())
    }

    pub(crate) fn retire_scope(&mut self, scope: crate::scope::WireSceneScope) {
        let bytes = &mut self.bytes;
        self.entries.retain(|entry| {
            let owned = match &entry.envelope {
                SyncEnvelope::Ownership(message) => message.scope == scope,
                SyncEnvelope::JournalEntry(message) => message.scope == scope,
                SyncEnvelope::JournalBatch(messages) => {
                    messages.iter().any(|message| message.scope == scope)
                }
                _ => false,
            };
            if owned {
                *bytes -= entry.bytes;
            }
            !owned
        });
    }
}

#[cfg(test)]
mod deferred_scene_tests {
    use super::*;

    #[test]
    fn retired_scope_releases_only_its_byte_budget_and_take_is_independent_of_new_traffic() {
        let mut pending = DeferredSceneMessages::default();
        let scope_a = crate::scope::WireSceneScope::Twin { mount_id: 1 };
        let scope_b = crate::scope::WireSceneScope::Twin { mount_id: 2 };
        let a = SyncEnvelope::JournalEntry(crate::journal_plane::JournalEntryMsg {
            scope: scope_a,
            json: "first".into(),
        });
        let b = SyncEnvelope::JournalEntry(crate::journal_plane::JournalEntryMsg {
            scope: scope_b,
            json: "replacement".into(),
        });
        let b_bytes = crate::codec::serialize_env(&b).unwrap().len();
        pending.admit(SessionId::LOCAL, a).unwrap();
        pending.admit(SessionId::LOCAL, b).unwrap();
        assert!(pending.bytes > b_bytes);
        pending.retire_scope(scope_a);
        assert_eq!(pending.bytes, b_bytes);
        let live = Some(lunco_core_session::ReplicationScope::Twin(
            lunco_workspace::TwinId::new(2),
        ));
        assert!(
            !pending.needs_retry(None),
            "a missing scene must not re-encode the queue every frame"
        );
        assert!(
            pending.needs_retry(live),
            "scene admission retries without new traffic"
        );
        let ready = pending.take(live);
        assert_eq!(ready.len(), 1);
        assert!(
            matches!(&ready[0].1, SyncEnvelope::JournalEntry(message) if message.scope == scope_b)
        );
        assert!(pending.is_empty());
        assert_eq!(pending.bytes, 0);
        assert!(!pending.needs_retry(live));

        let over_limit = SyncEnvelope::JournalEntry(crate::journal_plane::JournalEntryMsg {
            scope: scope_b,
            json: "x".repeat(crate::codec::MAX_ENVELOPE_BYTES),
        });
        assert!(pending.admit(SessionId::LOCAL, over_limit).is_err());
        assert!(pending.is_empty());
        assert_eq!(pending.bytes, 0);
    }
}

/// Connection whose version/author handshake has been admitted.
#[derive(Resource, Default)]
pub(crate) struct HandshakenConnection(pub Option<Entity>);
