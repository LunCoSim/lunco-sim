//! Client-side HTTP **bytes plane** — fetch scenario assets by CID.
//!
//! The manifest (which CIDs a scenario needs, and where its bytes live) arrives on
//! the authenticated QUIC session; the blobs themselves are pulled over plain HTTP
//! from `<asset_base_url><cid>`.
//!
//! ## Why not stream the bytes through the game session?
//!
//! lightyear's reliable sender never rejects a message: `buffer_send` unconditionally
//! inserts into `unacked_messages` and resends until acked. `MAX_CHUNKS_PER_FRAME`
//! throttles *queueing*, not *delivery*, so any transfer larger than the link can
//! drain grows the backlog without bound until delivery stalls — observed on a 40 MB
//! twin: the client wedged at ~12 MB while the host's queue passed 27 k chunks. HTTP
//! gets the OS's flow control for free, and a stalled download can't take the
//! simulation session down with it.
//!
//! ## Trust
//!
//! The endpoint is untrusted: every fetched blob is re-hashed and must match the CID
//! we asked for (`cid_for_content(&bytes) == cid`), so a hostile or misconfigured
//! server can withhold bytes but never substitute them. A failed fetch (network,
//! 404, hash mismatch) drops the CID from `requested` so it retries on the next
//! manifest — and if HTTP is unavailable entirely, the caller falls back to the
//! in-session chunk path.

use bevy::prelude::*;
use crossbeam_channel::{unbounded, Receiver, Sender};

use lunco_core::NetworkRole;

use crate::scenario::{cid_for_content, RemoteScenarioManifest};
use crate::scenario_sync::{asset_storage_handle, AssetDownloads, AssetPersist};

/// Max asset fetches in flight at once. Bounded so a many-file scenario doesn't open
/// dozens of sockets (and, on wasm, doesn't queue dozens of `fetch()` promises); the
/// per-connection HTTP flow control does the rest.
const MAX_INFLIGHT_FETCHES: usize = 4;

/// A completed (or failed) HTTP asset fetch, reported back to the Bevy world.
pub(crate) struct FetchOutcome {
    pub cid: Vec<u8>,
    /// Verified bytes, or `None` if the fetch or the hash check failed.
    pub bytes: Option<Vec<u8>>,
}

/// How many times to retry a single asset before giving up on it for this manifest
/// revision. Covers a transient network blip; does NOT paper over a server that
/// serves bytes which don't match their CID (e.g. a file the host is still writing)
/// — that fails identically every time, and retrying it forever is a hot loop.
const MAX_ATTEMPTS: u32 = 3;

/// Client-side channel carrying HTTP fetch results (sibling of `AssetPersist`).
#[derive(Resource)]
pub struct AssetHttpFetch {
    tx: Sender<FetchOutcome>,
    rx: Receiver<FetchOutcome>,
    /// Number of fetches currently in flight (bounded by `MAX_INFLIGHT_FETCHES`).
    inflight: usize,
    /// Failed attempts per CID. A CID that exhausts `MAX_ATTEMPTS` is left in
    /// `requested` — i.e. never re-issued — so the scenario simply never reaches
    /// `all_cached` instead of spinning. Cleared when a new revision lands.
    attempts: std::collections::HashMap<Vec<u8>, u32>,
    /// The revision `attempts` refers to.
    revision: Option<[u8; 32]>,
}

impl Default for AssetHttpFetch {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self { tx, rx, inflight: 0, attempts: Default::default(), revision: None }
    }
}

/// Client: pull missing assets over HTTP when the host advertised an endpoint.
///
/// Runs in place of `request_missing_assets` (which speaks the QUIC chunk protocol)
/// whenever `asset_base_url` is present. Verified bytes are handed to the same
/// `submit_persist` path the chunk reassembler uses, so cache layout, the
/// duplicate-CID fan-out, and progress accounting are all shared.
pub fn fetch_missing_assets_http(
    role: Res<NetworkRole>,
    remote: Res<RemoteScenarioManifest>,
    probe: Res<crate::scenario_sync::CacheProbeState>,
    mut downloads: ResMut<AssetDownloads>,
    mut fetch: ResMut<AssetHttpFetch>,
    persist: Res<AssetPersist>,
) {
    if role.is_host() {
        return;
    }
    let Some(manifest) = remote.manifest.as_ref() else {
        return;
    };
    // TODO(multiplayer): deferred — singleplayer focus for now, RBAC disabled for
    // ease of debugging. `asset_base_url` is host-advertised and unconstrained —
    // attacker-chosen outbound GET; constrain to the connected host's origin.
    // Revisit before multiplayer hardening (REVIEW-2026-07-19.md §2 Security,
    // LOW-MED).
    let Some(base) = manifest.asset_base_url.as_deref() else {
        return; // no HTTP endpoint → the QUIC chunk path handles this scenario
    };
    // Same gate as the chunk path: don't fetch what a prior session already cached.
    if !probe.settled_for(manifest.revision) {
        return;
    }
    // A new scenario revision retires the old failure tally.
    if fetch.revision != Some(manifest.revision) {
        fetch.revision = Some(manifest.revision);
        fetch.attempts.clear();
    }

    // Drain finished fetches first, freeing slots for the issue loop below.
    while let Ok(outcome) = fetch.rx.try_recv() {
        fetch.inflight = fetch.inflight.saturating_sub(1);
        let Some(bytes) = outcome.bytes else {
            // Retry a few times (transient network), then give up on this asset for
            // this revision. Leaving the CID in `requested` is what stops the retry:
            // the issue loop below skips it, so `all_cached` stays false and the
            // scene simply doesn't load — loudly, once, instead of a hot loop.
            let n = fetch.attempts.entry(outcome.cid.clone()).or_insert(0);
            *n += 1;
            if *n < MAX_ATTEMPTS {
                downloads.forget_requested(&outcome.cid);
            } else {
                error!(
                    "[net] giving up on scenario asset after {MAX_ATTEMPTS} attempts \
                     (bytes never matched their CID) — scenario will not finish loading"
                );
            }
            continue;
        };
        // Fan out to EVERY manifest path carrying this CID (byte-identical files
        // share one content id), exactly as the chunk path does.
        let targets: Vec<_> = manifest
            .assets
            .iter()
            .filter(|a| a.cid == outcome.cid)
            .filter_map(|a| asset_storage_handle(&manifest.scenario_id, &a.path))
            .collect();
        if targets.is_empty() {
            warn!("[net] fetched asset has no manifest entry / safe path; discarding");
            downloads.forget_requested(&outcome.cid);
            continue;
        }
        downloads.expect_writes(outcome.cid.clone(), targets.len());
        for handle in targets {
            crate::scenario_sync::submit_persist(
                persist.tx.clone(),
                outcome.cid.clone(),
                handle,
                bytes.clone(),
            );
        }
    }

    // Issue new fetches up to the concurrency cap.
    for asset in &manifest.assets {
        if fetch.inflight >= MAX_INFLIGHT_FETCHES {
            break;
        }
        if downloads.is_requested(&asset.cid) {
            continue;
        }
        let Some(cid) = crate::scenario::cid_from_bytes(&asset.cid) else {
            continue;
        };
        downloads.mark_requested(asset.cid.clone());
        fetch.inflight += 1;
        let url = format!("{base}{cid}");
        spawn_fetch(fetch.tx.clone(), asset.cid.clone(), url);
    }
}

/// Fetch `url`, verify the bytes hash back to `cid`, and report the outcome.
/// Fail-closed: any error, or a hash that doesn't match, yields `bytes: None`.
fn spawn_fetch(tx: Sender<FetchOutcome>, cid: Vec<u8>, url: String) {
    #[cfg(not(target_arch = "wasm32"))]
    bevy::tasks::IoTaskPool::get()
        .spawn(async move {
            let fetched = fetch_bytes_native(&url);
            let _ = tx.send(verified(cid, url, fetched));
        })
        .detach();
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let fetched = fetch_bytes_web(&url).await;
        let _ = tx.send(verified(cid, url, fetched));
    });
}

/// Turn a fetch result into an outcome, accepting the bytes ONLY if they hash back
/// to the CID we asked for. The endpoint is untrusted: it can withhold bytes, but it
/// cannot substitute them.
fn verified(cid: Vec<u8>, url: String, fetched: Result<Vec<u8>, String>) -> FetchOutcome {
    let bytes = match fetched {
        Ok(bytes) if cid_for_content(&bytes).to_bytes() == cid => Some(bytes),
        Ok(_) => {
            warn!("[net] asset {url} failed CID verification; discarding");
            None
        }
        Err(e) => {
            warn!("[net] fetch {url}: {e}");
            None
        }
    };
    FetchOutcome { cid, bytes }
}

/// Native byte-GET. Blocking (`ureq`) — hence the `IoTaskPool`, not the compute pool.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_bytes_native(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_body().into_reader(), &mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Web byte-GET, cache-first-forever in a Cache-Storage bucket: the blob is
/// content-addressed, so it can never go stale, and a page reload re-hydrates it
/// without touching the network.
///
/// The URL must be **same-origin** — `web_fetch` sets `RequestMode::SameOrigin`, and
/// an https page cannot fetch `http://host:5889` anyway (mixed content). Deployments
/// therefore reverse-proxy `/scenario-assets/` to the host's asset port and set
/// `LUNCO_ASSET_BASE_URL=/scenario-assets/`. Not `/assets/` — that is bevy's web
/// asset root, and proxying it away 404s the whole app.
#[cfg(target_arch = "wasm32")]
async fn fetch_bytes_web(url: &str) -> Result<Vec<u8>, String> {
    lunco_assets::web_fetch::fetch_bytes_cached("lunco-scenario-assets-v1", url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trust boundary of the whole hybrid transport: the HTTP endpoint is
    /// untrusted, so [`verified`] accepts fetched bytes ONLY when they re-hash to
    /// the CID we asked for. A server can therefore withhold bytes (or serve the
    /// wrong ones) but can never make the client accept a substitution.
    #[test]
    fn verified_accepts_only_bytes_matching_the_requested_cid() {
        let content = b"the real asset bytes".to_vec();
        let cid = cid_for_content(&content).to_bytes();

        // Correct bytes → accepted.
        let ok = verified(cid.clone(), "http://h/a".into(), Ok(content.clone()));
        assert_eq!(ok.cid, cid);
        assert_eq!(ok.bytes.as_deref(), Some(content.as_slice()));

        // Substituted bytes (a hostile/misconfigured server) → rejected, CID kept
        // so the caller can retry/fall back rather than cache a lie.
        let substituted = verified(cid.clone(), "http://h/a".into(), Ok(b"evil".to_vec()));
        assert_eq!(substituted.cid, cid);
        assert!(substituted.bytes.is_none(), "mismatched content must be discarded");

        // Even empty bytes must not pass for a non-empty CID.
        let empty = verified(cid.clone(), "http://h/a".into(), Ok(Vec::new()));
        assert!(empty.bytes.is_none());

        // A transport error → no bytes, CID preserved for retry.
        let errored = verified(cid.clone(), "http://h/a".into(), Err("404".into()));
        assert_eq!(errored.cid, cid);
        assert!(errored.bytes.is_none());
    }

    /// Two different contents produce different CIDs, so a blob fetched for one
    /// CID cannot satisfy another (no cross-asset confusion).
    #[test]
    fn verified_rejects_valid_bytes_offered_under_the_wrong_cid() {
        let a = b"asset A".to_vec();
        let b = b"asset B".to_vec();
        let cid_a = cid_for_content(&a).to_bytes();
        let cid_b = cid_for_content(&b).to_bytes();
        assert_ne!(cid_a, cid_b);

        // B's real bytes offered where A was requested → rejected.
        let wrong = verified(cid_a.clone(), "http://h/a".into(), Ok(b));
        assert!(wrong.bytes.is_none());
    }
}
