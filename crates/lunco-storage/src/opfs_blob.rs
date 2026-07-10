//! Wasm async blob store for the precompute cache tier — OPFS-backed.
//!
//! Mirrors `lunco-precompute`'s `<namespace>/<key-hex>` entry layout under a
//! `precompute/` OPFS root, one blob per entry. `lunco-precompute`'s **sync**
//! fs tier is native-only (on wasm the sync path would route to
//! `localStorage`-as-hex — 2× size, quota-bound), so wasm consumers integrate
//! *this* async tier at their own async seams instead.
//!
//! Thin policy layer over [`crate::OpfsStorage`]'s inherent async methods —
//! the futures hold non-`Send` JS values, so callers drive these with
//! `wasm_bindgen_futures::spawn_local` (see the `opfs_storage` module docs).

use std::path::PathBuf;

use crate::{OpfsStorage, StorageHandle};

/// The OPFS path for one cache entry: `precompute/<namespace>/<key-hex>`.
/// `namespace` is `/`-separated (e.g. `"terrain/dem-grid"`), matching
/// `lunco_precompute::entry_dir`'s segmenting.
fn handle(namespace: &str, key_hex: &str) -> StorageHandle {
    let mut path = PathBuf::from("precompute");
    for seg in namespace.split('/').filter(|s| !s.is_empty()) {
        path.push(seg);
    }
    path.push(key_hex);
    StorageHandle::File(path)
}

/// Read the blob stored under `precompute/<namespace>/<key-hex>`.
/// `None` on any miss/error — the caller treats it as a cache miss.
pub async fn read(namespace: &str, key_hex: &str) -> Option<Vec<u8>> {
    OpfsStorage::new().read(&handle(namespace, key_hex)).await.ok()
}

/// Best-effort write of `bytes` under `precompute/<namespace>/<key-hex>`.
/// A failed write only costs a rebake next load, so it logs and continues —
/// never surfaces an error to the caller.
pub async fn write(namespace: &str, key_hex: &str, bytes: &[u8]) {
    if let Err(e) = OpfsStorage::new().write(&handle(namespace, key_hex), bytes).await {
        web_sys::console::warn_1(
            &format!("[opfs-blob] write {namespace}/{key_hex} failed: {e}").into(),
        );
    }
}
