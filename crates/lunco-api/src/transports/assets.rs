//! Read-only, content-addressed asset server (native, `transport-http`).
//!
//! Serves `GET /scenario-assets/<cid>` — the **bytes plane** of scenario
//! distribution. (Deliberately not `/assets/`: that is bevy's web asset root, from
//! which the wasm bundle serves its own shaders/scenes/models. A same-origin
//! deployment proxying `/assets/` to this listener would swallow all of them.) The
//! manifest (which CIDs a scenario needs) still travels the authenticated QUIC
//! session; only the opaque blobs ride HTTP, because lightyear's reliable sender
//! queues without bound and streaming a multi-MB twin through it stalls the
//! session outright.
//!
//! ## Why this is safe to bind on `0.0.0.0`
//!
//! Unlike the command API (`/api/commands`, loopback-only — it executes arbitrary
//! commands), this listener is:
//! - **read-only** — no state is mutated, no command is dispatched;
//! - **content-addressed** — the only key is a CID, so a caller can fetch a blob
//!   *only if it already knows that blob's hash*, which it learns from the
//!   authenticated manifest. There is no directory listing and no path input, so
//!   no traversal surface: an unknown CID is a flat 404;
//! - **integrity-checked at the consumer** — the client re-hashes every byte back
//!   to the CID it asked for (`reassemble`/`http_fetch` fail closed), so a hostile
//!   or buggy server cannot substitute content.
//!
//! It therefore lives on its own port, separate from the command API, and never
//! shares that server's bind address or router.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
};

/// Canonical CID string (base32 `bafk…`) → absolute path of the blob on the host.
/// Written by the scenario-manifest build, read by the HTTP handler on a tokio
/// worker thread — hence the lock. Cheap: swapped wholesale once per manifest.
pub type AssetIndex = Arc<RwLock<HashMap<String, PathBuf>>>;

/// Default port for the asset listener. Distinct from the command API (4101) —
/// this one is meant to be reachable, that one is not.
pub const DEFAULT_ASSET_PORT: u16 = 5889;

/// Spawn the asset server on `addr` (e.g. `0.0.0.0:5889`), serving blobs named by
/// the CIDs in `index`.
///
/// Binding a public interface is safe here (see module docs) and is the default so
/// native/LAN peers can fetch directly. A deployment that fronts this with nginx
/// should bind `127.0.0.1` instead (`LUNCO_ASSET_BIND`) so the only public door is
/// the proxy. Runs on its own OS thread + tokio runtime, mirroring `spawn_server`;
/// a bind failure is logged and the thread returns (clients then fall back to the
/// in-session chunk path).
#[allow(clippy::disallowed_methods)]
pub fn spawn_asset_server(addr: String, index: AssetIndex) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                bevy::log::error!("[lunco-api] asset server runtime failed to start: {e}");
                return;
            }
        };
        rt.block_on(async move {
            // NOT `/assets/` — that is bevy's web asset root, where the wasm bundle
            // serves its own shaders/scenes/models. A same-origin deployment
            // proxying `/assets/` here would swallow every one of them.
            let app = axum::Router::new()
                .route("/scenario-assets/{cid}", axum::routing::get(serve_asset))
                .with_state(index);

            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    bevy::log::error!(
                        "[lunco-api] asset server failed to bind {addr}: {e} \
                         (port in use?) — clients will fall back to in-session asset streaming"
                    );
                    return;
                }
            };
            bevy::log::info!("[lunco-api] asset server listening on {addr}");
            if let Err(e) = axum::serve(listener, app).await {
                bevy::log::error!("[lunco-api] asset server stopped with error: {e}");
            }
        });
    });
}

/// `GET /scenario-assets/<cid>` → the blob's bytes, or 404.
///
/// `cid` is used purely as a map key — never as a path component — so a crafted
/// value (`../…`, absolute paths) can only miss the map. Blobs are immutable by
/// construction (the name *is* the hash), so they're cacheable forever.
async fn serve_asset(State(index): State<AssetIndex>, Path(cid): Path<String>) -> impl IntoResponse {
    let path = {
        let Ok(map) = index.read() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        map.get(&cid).cloned()
    };
    let Some(path) = path else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                // Content-addressed ⇒ immutable. Also lets a browser reuse the blob
                // across reloads without revalidating.
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
                // The wasm client is served from a different origin (nginx) than this
                // listener; without this the browser refuses to hand us the body.
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            bevy::log::warn!("[lunco-api] asset {cid} unreadable at {path:?}: {e}");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

// Test fixtures live on disk and run natively only — the workspace ban on
// `std::fs` guards *wasm runtime* code paths, not tests (clippy.toml says so;
// cargo has no path-scoped lint config, so the exemption is written out here).
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn index_with(entries: &[(&str, PathBuf)]) -> AssetIndex {
        let map: HashMap<String, PathBuf> =
            entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        Arc::new(RwLock::new(map))
    }

    /// The security property: `cid` is only ever a MAP KEY, never joined onto a
    /// filesystem path. A crafted value (`../…`, an absolute path, a real file on
    /// disk) that isn't an advertised key can therefore only miss the map → 404.
    /// It can never escape to read arbitrary files.
    #[tokio::test]
    async fn crafted_cids_can_only_miss_the_map_never_traverse() {
        // The map advertises exactly one blob, under its CID key.
        let dir = std::env::temp_dir().join("lunco-asset-serve-test");
        std::fs::create_dir_all(&dir).unwrap();
        let blob = dir.join("blob.bin");
        std::fs::write(&blob, b"real blob").unwrap();
        let index = index_with(&[("bafkreirealcid", blob.clone())]);

        // A traversal string and an absolute path to a genuinely existing file are
        // NOT keys → 404, even though the file exists and is readable.
        for hostile in ["../../../../etc/passwd", "/etc/passwd", "bafk-not-advertised"] {
            let resp = serve_asset(State(index.clone()), Path(hostile.to_string()))
                .await
                .into_response();
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "crafted cid {hostile:?} must miss the map, not resolve a path"
            );
        }

        // The advertised key resolves to its blob.
        let ok = serve_asset(State(index), Path("bafkreirealcid".to_string()))
            .await
            .into_response();
        assert_eq!(ok.status(), StatusCode::OK);

        let _ = std::fs::remove_file(&blob);
    }

    /// A key present in the map but whose backing file has gone missing degrades
    /// to 404, not a panic or a 500.
    #[tokio::test]
    async fn advertised_key_with_missing_file_is_404() {
        let index = index_with(&[("bafkreighost", PathBuf::from("/no/such/blob/on/disk"))]);
        let resp = serve_asset(State(index), Path("bafkreighost".to_string()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
