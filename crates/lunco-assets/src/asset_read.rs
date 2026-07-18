//! Reading a **shipped asset's bytes**, uniformly on native and web.
//!
//! [`discovery`](crate::discovery) answers *which* files the project has.
//! This answers *what is in one of them* — the other half a catalogue needs, and
//! the half that used to be unavailable in the browser.
//!
//! # Native and web read the same file
//!
//! - **Native** — through [`lunco_storage::FileStorage`], the I/O chokepoint.
//!   Not `std::fs`: this crate is on the `disallowed_methods` allow-list, but
//!   being allowed to reach past the storage layer is not a reason to.
//! - **Web** — a same-origin `fetch` of `assets/<rel>`, cached in the browser's
//!   Cache Storage via [`web_fetch`](crate::web_fetch). **That is the exact URL
//!   Bevy's `AssetServer` uses to load the same file when it is spawned** — the
//!   bytes were always served and always reachable. What was missing was a
//!   caller willing to be async.
//!
//! # Why this is not a `Storage` impl
//!
//! [`lunco_storage::Storage`] is `Send + Sync`, and browser `fetch` futures are
//! `!Send` — a `web_sys` `Promise` is bound to the JS event loop and cannot
//! cross a thread. That is not a wrinkle we can paper over; it is why
//! [`OpfsStorage`](lunco_storage::opfs_storage) also exposes inherent async
//! methods rather than implementing the trait. So the web side is a free async
//! fn, and the native side routes through `FileStorage` — the trait still owns
//! every byte it can own.

use crate::discovery::AssetFile;

/// Cache-Storage bucket for the shipped engine library. Versioned like the other
/// buckets (`lunco-msl-v1`, `lunco-twin-v1`) so a format change can invalidate
/// it wholesale.
#[cfg(target_arch = "wasm32")]
pub const ASSET_CACHE_BUCKET: &str = "lunco-assets-v1";

/// Read a discovered asset's bytes.
///
/// `Err` on a missing file / failed fetch — callers decide what an unreadable
/// asset means (the catalogue treats it as "not a part", never as a default).
#[cfg(not(target_arch = "wasm32"))]
pub async fn read_asset_bytes(asset: &AssetFile) -> Result<Vec<u8>, String> {
    use lunco_storage::{FileStorage, Storage, StorageHandle};
    FileStorage::new()
        .read(&StorageHandle::File(asset.abs_path.clone()))
        .await
        .map_err(|e| format!("{}: {e}", asset.abs_path.display()))
}

/// Web counterpart of [`read_asset_bytes`] — see that fn's docs.
///
/// The engine library is served at `assets/<rel>` next to the wasm bundle. Uses
/// the cache-first fetch: a `*.usda` is small, immutable for the life of a
/// deployed build, and re-fetching it on every boot would be a needless round
/// trip per asset.
#[cfg(target_arch = "wasm32")]
pub async fn read_asset_bytes(asset: &AssetFile) -> Result<Vec<u8>, String> {
    let url = crate::asset_path::web_url(&asset.rel);
    crate::web_fetch::fetch_bytes_cached(ASSET_CACHE_BUCKET, &url).await
}

/// Read a discovered asset's bytes as UTF-8 text (`*.usda`, `*.wgsl`).
pub async fn read_asset_text(asset: &AssetFile) -> Result<String, String> {
    let bytes = read_asset_bytes(asset).await?;
    String::from_utf8(bytes).map_err(|e| format!("{}: not UTF-8: {e}", asset.rel))
}
