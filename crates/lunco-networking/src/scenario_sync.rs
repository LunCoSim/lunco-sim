//! Scenario **asset transfer** — Phase 3 of scenario distribution (the bytes).
//!
//! Phase 1 ([`crate::scenario`]) publishes the manifest: "scenario X at revision
//! R with these asset CIDs". This module moves the actual **bytes**, one-way
//! host → client, so a joined client can materialise the scenario in its local
//! cache (`<cache_dir>/scenarios/<scenario_id>/<path>`). It is deliberately the
//! *content plane* only — opaque bytes addressed by CID, verified by re-hashing,
//! no merge (documents merge via the journal; see `NETWORKING_ASSET_SYNC_DESIGN.md`).
//!
//! Flow:
//! - **client** ([`request_missing_assets`]): when a new manifest lands, diff its
//!   asset CIDs against what we've already fetched this session and emit one
//!   [`AssetRequestMsg`](crate::scenario::AssetRequestMsg) for the missing set on
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
//! Deferred (documented, not silent): explicit flow-control/backpressure beyond a
//! per-frame send cap; the `AssetHave` dedupe hint; cross-session on-disk
//! cache-hit detection (needs a cheap sync `exists`/metadata storage API — today
//! a restarted client re-fetches); off-threading the client-side verify+write of
//! a completed large asset; and Phase 4 (loading the scene once assets land).

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use crossbeam_channel::{unbounded, Receiver, Sender};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use lunco_core::{NetworkRole, SessionId, SyncChannel};
use lunco_storage::StorageHandle;

use crate::scenario::{cid_from_bytes, AssetChunkMsg, AssetRequestMsg, RemoteScenarioManifest, ScenarioManifestMsg};
use crate::sync::{SyncEnvelope, SyncOutbox};

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
    /// disk/OPFS and a [`scenario_asset_uri`] load will resolve. `false` for an
    /// empty manifest (nothing to consume).
    pub fn all_cached(&self, manifest: &ScenarioManifestMsg) -> bool {
        !manifest.assets.is_empty()
            && manifest.assets.iter().all(|a| self.completed.contains(&a.cid))
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

/// One per-asset record persisted in `<cache_root>/.scenario.json`. The probe keys
/// cache-hits on `cid` — not file presence — so a twin whose content changed at a
/// path is re-fetched, never served stale; the cached-twin menu reads the same
/// file for name/size/scene.
#[derive(serde::Serialize, serde::Deserialize)]
struct ScenarioIndexAsset {
    path: String,
    cid: Vec<u8>,
    size: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ScenarioIndex {
    name: String,
    default_scene: Option<String>,
    revision: [u8; 32],
    total_bytes: u64,
    assets: Vec<ScenarioIndexAsset>,
}

/// `<cache_root>/.scenario.json` — the per-scenario "what's cached" marker.
fn scenario_index_path(scenario_id: &[u8; 16]) -> PathBuf {
    scenario_cache_root(scenario_id).join(".scenario.json")
}

/// One cached scenario, listed in the top-level `<cache>/scenarios/index.json`
/// and surfaced in the cached-twins menu (G3). Persisted across sessions so the
/// menu can list + load downloaded twins with no server connected.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CachedTwinSummary {
    pub scenario_id: [u8; 16],
    pub name: String,
    pub default_scene: Option<String>,
    pub total_bytes: u64,
    pub revision: [u8; 32],
}

/// The cached-twins menu's data: every scenario fully downloaded to this peer's
/// cache. Rebuilt from `index.json` at boot ([`refresh_cached_twins_registry`])
/// and kept current by [`write_scenario_index`] as new downloads complete.
#[derive(Resource, Default)]
pub struct CachedTwinsRegistry {
    pub entries: Vec<CachedTwinSummary>,
}

/// Channel for the async `index.json` read at boot to report back to
/// [`refresh_cached_twins_registry`] (sibling of [`AssetPersist`] /
/// [`AssetCacheProbe`]).
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

/// `<cache>/scenarios/index.json` — the top-level list of cached scenarios.
fn scenarios_index_path() -> PathBuf {
    lunco_assets::cache_dir().join("scenarios").join("index.json")
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
/// manifest so the import redistributes.
#[derive(Resource, Default)]
pub struct PendingAssetOffers(pub Vec<crate::scenario::AssetOfferMsg>);

/// Client → host: offer an asset the local peer just imported so the host writes it
/// into the shared twin and redistributes it (the bidirectional counterpart of the
/// host serve). Computes the CID locally; the host re-verifies. Pushes onto the
/// [`SyncOutbox`] over the reliable `BulkData` lane. No-op if the payload is empty.
///
/// TODO(bidirectional-content): wire the call site — fire this from the actual
/// import surface (file-open / drag-drop / palette add) when a NEW asset enters the
/// twin. Today it's the mechanism, not yet the trigger. Also cap `bytes` and chunk
/// large offers (see [`AssetOfferMsg`](crate::scenario::AssetOfferMsg)).
pub fn offer_asset_to_host(outbox: &mut crate::sync::SyncOutbox, path: impl Into<String>, bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    let cid = crate::scenario::cid_for_content(&bytes).to_bytes();
    outbox.0.push((
        lunco_core::SyncChannel::BulkData,
        crate::sync::SyncEnvelope::AssetOffer(crate::scenario::AssetOfferMsg {
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

/// Root of a scenario's local asset cache: `<cache_dir>/scenarios/<hex id>/`.
pub fn scenario_cache_root(scenario_id: &[u8; 16]) -> PathBuf {
    lunco_assets::cache_dir().join("scenarios").join(hex16(scenario_id))
}

/// A safe *relative* `PathBuf` from a `/`-separated manifest asset path,
/// **rejecting traversal** (empty / `.` / `..` / backslash segments) — the path
/// comes from a remote host and must never escape a target root. `None` if unsafe
/// or empty.
pub(crate) fn safe_rel_path(rel: &str) -> Option<PathBuf> {
    let mut p = PathBuf::new();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') {
            warn!("[net] rejecting unsafe scenario asset path: {rel:?}");
            return None;
        }
        p.push(seg);
    }
    (!p.as_os_str().is_empty()).then_some(p)
}

/// Resolve a manifest asset's relative path to its on-disk cache location under
/// [`scenario_cache_root`], traversal-guarded via [`safe_rel_path`].
fn scenario_asset_path(scenario_id: &[u8; 16], rel: &str) -> Option<PathBuf> {
    Some(scenario_cache_root(scenario_id).join(safe_rel_path(rel)?))
}

fn hex16(b: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
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
    probe_state: Res<CacheProbeState>,
    mut downloads: ResMut<AssetDownloads>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if role.is_host() {
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
        info!("[net] requesting {} missing scenario asset(s)", missing.len());
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
    persist: Res<AssetPersist>,
) {
    if role.is_host() || incoming.0.is_empty() {
        return;
    }
    for ch in std::mem::take(&mut incoming.0) {
        // Append into the per-CID buffer + feed the running hash (scoped borrow so
        // we can touch `downloads.requested` afterwards without overlapping it).
        let (complete, out_of_order) = {
            let entry = downloads.inflight.entry(ch.cid.clone()).or_default();
            entry.total = ch.total;
            if ch.offset != entry.buf.len() as u64 {
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
        // materialized, or a `scenario://` load misses the duplicate's second path.
        let targets: Vec<StorageHandle> = remote
            .manifest
            .as_ref()
            .map(|m| {
                m.assets
                    .iter()
                    .filter(|a| a.cid.as_slice() == ch.cid.as_slice())
                    .filter_map(|a| asset_storage_handle(&m.scenario_id, &a.path))
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
        downloads.pending_writes.insert(ch.cid.clone(), targets.len());
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
    persist: Res<AssetPersist>,
    mut downloads: ResMut<AssetDownloads>,
) {
    if role.is_host() {
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
        match downloads.pending_writes.get_mut(&outcome.cid) {
            Some(remaining) => {
                *remaining -= 1;
                if *remaining == 0 {
                    downloads.pending_writes.remove(&outcome.cid);
                    downloads.completed.insert(outcome.cid);
                }
            }
            // No tally (e.g. a failure already cleared it) → ignore the straggler.
            None => {}
        }
    }
}

// ── Client: cross-session cache-hit probe (G1) ─────────────────────────────────

/// Read the scenario index, if present and well-formed. `None` if missing or
/// unreadable — treated as "nothing recognized", so assets are fetched afresh.
async fn read_scenario_index(scenario_id: &[u8; 16]) -> Option<ScenarioIndex> {
    let handle = StorageHandle::File(scenario_index_path(scenario_id));
    let bytes = storage_read(&handle).await?;
    serde_json::from_slice(&bytes).ok()
}

/// True iff the cache file for an asset is present on disk/OPFS.
async fn cached_asset_exists(path: &std::path::Path) -> bool {
    storage_exists(&StorageHandle::File(path.to_path_buf())).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn storage_read(handle: &StorageHandle) -> Option<Vec<u8>> {
    use lunco_storage::Storage;
    lunco_storage::FileStorage::new().read(handle).await.ok()
}
#[cfg(target_arch = "wasm32")]
async fn storage_read(handle: &StorageHandle) -> Option<Vec<u8>> {
    lunco_storage::OpfsStorage::new().read(handle).await.ok()
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
) -> ProbeOutcome {
    let by_path: HashMap<String, Vec<u8>> = read_scenario_index(&scenario_id)
        .await
        .map(|idx| idx.assets.into_iter().map(|a| (a.path, a.cid)).collect())
        .unwrap_or_default();
    let mut cached = HashSet::new();
    for (cid, rel) in &assets {
        if by_path.get(rel).is_some_and(|c| c == cid) {
            if let Some(p) = scenario_asset_path(&scenario_id, rel) {
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
    probe: Res<AssetCacheProbe>,
    mut state: ResMut<CacheProbeState>,
    mut downloads: ResMut<AssetDownloads>,
) {
    if role.is_host() {
        return;
    }
    if let Some(m) = remote.manifest.as_ref() {
        if state.kicked != Some(m.revision) {
            state.kicked = Some(m.revision);
            let scenario_id = m.scenario_id;
            let revision = m.revision;
            let assets: Vec<(Vec<u8>, String)> =
                m.assets.iter().map(|a| (a.cid.clone(), a.path.clone())).collect();
            let tx = probe.tx.clone();
            let fut = async move {
                let outcome = run_cache_probe(scenario_id, revision, assets).await;
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

/// Client: once a scenario is fully cached, persist its `.scenario.json` index so
/// a later session's probe recognizes it and the cached-twin menu can list it.
/// Fires once per revision. Client-only.
pub fn write_scenario_index(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    downloads: Res<AssetDownloads>,
    mut registry: ResMut<CachedTwinsRegistry>,
    mut written: Local<Option<[u8; 32]>>,
) {
    if role.is_host() {
        return;
    }
    let Some(m) = remote.manifest.as_ref() else {
        return;
    };
    if *written == Some(m.revision) || !downloads.all_cached(m) {
        return;
    }
    *written = Some(m.revision);
    let summary = CachedTwinSummary {
        scenario_id: m.scenario_id,
        name: m.name.clone(),
        default_scene: m.default_scene.clone(),
        total_bytes: m.assets.iter().map(|a| a.size).sum(),
        revision: m.revision,
    };
    // Keep the in-memory registry current so the menu updates live as a download
    // completes; the async block below persists both the per-scenario index and
    // the top-level index.json so a later boot can rebuild the registry.
    registry.entries.retain(|e| e.scenario_id != summary.scenario_id);
    registry.entries.push(summary.clone());
    let index = ScenarioIndex {
        name: m.name.clone(),
        default_scene: m.default_scene.clone(),
        revision: m.revision,
        total_bytes: summary.total_bytes,
        assets: m
            .assets
            .iter()
            .map(|a| ScenarioIndexAsset { path: a.path.clone(), cid: a.cid.clone(), size: a.size })
            .collect(),
    };
    let Ok(bytes) = serde_json::to_vec(&index) else {
        return;
    };
    let per_scenario = StorageHandle::File(scenario_index_path(&m.scenario_id));
    let top_index = StorageHandle::File(scenarios_index_path());
    let fut = async move {
        let _ = do_write(per_scenario, bytes).await;
        // Merge into the top-level index.json (read → replace this id → write).
        let mut entries: Vec<CachedTwinSummary> = match storage_read(&top_index).await {
            Some(b) => serde_json::from_slice(&b).unwrap_or_default(),
            None => Vec::new(),
        };
        entries.retain(|e| e.scenario_id != summary.scenario_id);
        entries.push(summary);
        if let Ok(json) = serde_json::to_vec(&entries) {
            let _ = do_write(top_index, json).await;
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    AsyncComputeTaskPool::get().spawn(fut).detach();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(fut);
}

/// G3 boot: read the top-level `index.json` once and rebuild
/// [`CachedTwinsRegistry`] so the cached-twins menu lists twins downloaded in a
/// prior session. Runs on every peer (host included — a host may have promoted
/// a scenario earlier); no-op after the first successful read.
pub fn refresh_cached_twins_registry(
    index: Res<CachedTwinsIndex>,
    mut registry: ResMut<CachedTwinsRegistry>,
    mut kicked: Local<bool>,
) {
    if !*kicked {
        *kicked = true;
        let tx = index.tx.clone();
        let fut = async move {
            let entries = match storage_read(&StorageHandle::File(scenarios_index_path())).await {
                Some(bytes) => serde_json::from_slice::<Vec<CachedTwinSummary>>(&bytes)
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let _ = tx.send(entries);
        };
        #[cfg(not(target_arch = "wasm32"))]
        AsyncComputeTaskPool::get().spawn(fut).detach();
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(fut);
    }
    if let Ok(entries) = index.rx.try_recv() {
        // Only adopt the on-disk list if we haven't already accumulated entries
        // this session (a download completed before the boot read landed).
        if registry.entries.is_empty() {
            registry.entries = entries;
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
    downloads: Res<AssetDownloads>,
    mut status: ResMut<ScenarioDownloadStatus>,
) {
    if role.is_host() {
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

/// The `scenario://` asset URI for a downloaded scenario asset (e.g. the entry
/// scene). Resolves through the `scenario` asset source to
/// `<cache_dir>/scenarios/<id>/<rel>` — where the download wrote it. Used by the
/// consumer (Phase 4) to `LoadScene` a fully-cached scenario.
pub fn scenario_asset_uri(scenario_id: &[u8; 16], rel: &str) -> String {
    format!("scenario://{}/{}", hex16(scenario_id), rel)
}

/// The storage handle for a scenario asset's cache location. A
/// [`StorageHandle::File`] on **both** platforms (native: absolute, under
/// `cache_dir()`; web: the same path fed to `OpfsStorage`, which maps its
/// components onto the OPFS tree) — so only the backend, not the handle, differs.
pub(crate) fn asset_storage_handle(scenario_id: &[u8; 16], rel: &str) -> Option<StorageHandle> {
    Some(StorageHandle::File(scenario_asset_path(scenario_id, rel)?))
}

/// Spawn the verify-passed asset's write on the platform's async executor and
/// report the outcome back over `tx`. The write NEVER runs on the calling
/// system: native → `AsyncComputeTaskPool` (real thread); web → `spawn_local`
/// (async OPFS on the main thread, non-blocking). The awaited body is the only
/// native/web divergence — see [`do_write`].
pub(crate) fn submit_persist(tx: Sender<PersistOutcome>, cid: Vec<u8>, handle: StorageHandle, bytes: Vec<u8>) {
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
    match lunco_storage::FileStorage::new().write(&handle, &bytes).await {
        Ok(()) => true,
        Err(e) => {
            warn!("[net] asset cache write failed: {e}");
            false
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn do_write(handle: StorageHandle, bytes: Vec<u8>) -> bool {
    match lunco_storage::OpfsStorage::new().write(&handle, &bytes).await {
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
        info!("[net] serving {} scenario asset(s) to session {:?}", jobs.len(), session);
        tasks.0.push((session, pool.spawn(async move { read_and_chunk(jobs) })));
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
            out.push(AssetChunkMsg { cid: cid.clone(), offset: 0, total: 0, data: Vec::new() });
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
    remote: Res<RemoteScenarioManifest>,
    mut workspace: ResMut<lunco_workspace::WorkspaceResource>,
    mut commands: Commands,
) {
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
/// into a `twin://` scene load, replacing the read-only `scenario://` view.
#[cfg(not(target_arch = "wasm32"))]
fn promote_scenario_to_folder(
    manifest: &ScenarioManifestMsg,
    folder: &str,
    workspace: &mut lunco_workspace::WorkspaceResource,
    commands: &mut Commands,
) {
    let target = PathBuf::from(folder);
    let cache_root = scenario_cache_root(&manifest.scenario_id);
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
    warn!("[promote] promotion needs a native filesystem folder; web (File System Access) is a TODO");
}

lunco_core::register_commands!(on_promote_scenario);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::cid_for_content;

    /// Two byte-identical files share ONE CID (the transfer is content-addressed,
    /// so the host streams those bytes once). The client must materialize the blob
    /// at EVERY manifest path that carries the CID — resolving with `.find()` wrote
    /// only the first, and a `scenario://` load then 404'd on the duplicate's path.
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
            .filter_map(|(path, _)| asset_storage_handle(&id, path))
            .collect();

        assert_eq!(targets.len(), 2, "both paths sharing the CID must be written");
        let expect = |rel: &str| StorageHandle::File(scenario_asset_path(&id, rel).unwrap());
        assert!(targets.contains(&expect("rover.glb")));
        assert!(targets.contains(&expect("structures/rover.glb")));
    }

    #[test]
    fn asset_path_rejects_traversal() {
        let id = [7u8; 16];
        assert!(scenario_asset_path(&id, "scenes/main.usda").is_some());
        assert!(scenario_asset_path(&id, "../escape").is_none());
        assert!(scenario_asset_path(&id, "a/../../b").is_none());
        assert!(scenario_asset_path(&id, "a//b").is_none()); // empty segment
    }

    #[test]
    fn read_and_chunk_slices_and_preserves_offsets() {
        // A file bigger than one chunk → multiple ordered chunks, contiguous offsets.
        let tmp = std::env::temp_dir().join("lunco_asset_chunk_test.bin");
        let data = vec![0xABu8; ASSET_CHUNK_SIZE + 123];
        lunco_storage::write_file_sync(&tmp, &data).unwrap();
        let cid = cid_for_content(&data).to_bytes();
        let chunks = read_and_chunk(vec![(cid.clone(), tmp.clone())]);
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
