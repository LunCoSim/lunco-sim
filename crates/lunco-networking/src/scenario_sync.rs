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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use lunco_core::{NetworkRole, SessionId, SyncChannel};

use crate::scenario::{cid_for_content, AssetChunkMsg, AssetRequestMsg, RemoteScenarioManifest, ScenarioManifestMsg};
use crate::sync::{SyncEnvelope, SyncOutbox};

/// Asset chunk payload size (bytes). Kept well under the lightyear reliable
/// fragment limit so a chunk never needs the transport's own fragmentation on
/// top of ours. 60 KiB leaves headroom for the envelope/frame overhead under a
/// typical 64 KiB message cap.
pub const ASSET_CHUNK_SIZE: usize = 60 * 1024;

/// Max asset chunks the host flushes to the wire per frame (crude backpressure so
/// a large multi-asset request can't dump thousands of fragments into lightyear's
/// send buffer in a single `Update`). Consumed by `server.rs`'s send system.
pub const MAX_CHUNKS_PER_FRAME: usize = 256;

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
}

#[derive(Default)]
struct Inflight {
    total: u64,
    buf: Vec<u8>,
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

/// Resolve a manifest asset's relative path to its on-disk cache location,
/// **rejecting path traversal** (`..`, absolute, or backslash segments) — the
/// path comes from a remote host and must never escape the scenario cache root.
fn scenario_asset_path(scenario_id: &[u8; 16], rel: &str) -> Option<PathBuf> {
    let mut p = scenario_cache_root(scenario_id);
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') {
            warn!("[net] rejecting unsafe scenario asset path: {rel:?}");
            return None;
        }
        p.push(seg);
    }
    Some(p)
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
    mut downloads: ResMut<AssetDownloads>,
    mut outbox: ResMut<SyncOutbox>,
) {
    if role.is_host() || !remote.is_changed() {
        return;
    }
    let Some(manifest) = remote.manifest.as_ref() else {
        return;
    };
    let mut missing = Vec::new();
    for asset in &manifest.assets {
        if downloads.requested.contains(&asset.cid) {
            continue;
        }
        // First sight of this CID this session → request it. (Cross-session
        // on-disk cache reuse is a documented follow-up; today a fresh client
        // re-fetches rather than probing the cache for a hit.)
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
) {
    if role.is_host() || incoming.0.is_empty() {
        return;
    }
    for ch in std::mem::take(&mut incoming.0) {
        // Append into the per-CID buffer (scoped borrow so we can touch
        // `downloads.requested` afterwards without overlapping the `entry` borrow).
        let (complete, out_of_order) = {
            let entry = downloads.inflight.entry(ch.cid.clone()).or_default();
            entry.total = ch.total;
            if ch.offset != entry.buf.len() as u64 {
                (false, true)
            } else {
                entry.buf.extend_from_slice(&ch.data);
                (entry.buf.len() as u64 >= entry.total, false)
            }
        };
        if out_of_order {
            warn!("[net] asset chunk out of order (cid); dropping partial download");
            downloads.inflight.remove(&ch.cid);
            downloads.requested.remove(&ch.cid); // allow a future re-request
            continue;
        }
        if complete {
            if let Some(done) = downloads.inflight.remove(&ch.cid) {
                if !finish_asset(&ch.cid, &done.buf, remote.manifest.as_ref()) {
                    downloads.requested.remove(&ch.cid); // failed → retriable
                }
            }
        }
    }
}

/// Verify a completed asset's bytes hash back to its CID (fail-closed) and write
/// it to the scenario cache. Returns `true` on success.
fn finish_asset(cid: &[u8], bytes: &[u8], manifest: Option<&ScenarioManifestMsg>) -> bool {
    if cid_for_content(bytes).to_bytes().as_slice() != cid {
        warn!("[net] downloaded asset failed CID verification; discarding");
        return false;
    }
    let Some(manifest) = manifest else {
        warn!("[net] asset completed but no manifest to place it; discarding");
        return false;
    };
    let Some(asset) = manifest.assets.iter().find(|a| a.cid.as_slice() == cid) else {
        warn!("[net] verified asset not in current manifest; discarding");
        return false;
    };
    let Some(path) = scenario_asset_path(&manifest.scenario_id, &asset.path) else {
        return false;
    };
    match lunco_storage::write_file_sync(&path, bytes) {
        Ok(()) => {
            info!("[net] scenario asset cached: {}", asset.path);
            true
        }
        Err(e) => {
            warn!("[net] failed to cache asset {}: {e}", asset.path);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
