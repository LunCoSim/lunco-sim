//! `scenario://` asset source — reads a networked client's **downloaded
//! scenario** assets out of the local cache (`<cache_dir>/scenarios/<id>/…`).
//!
//! Sibling of [`crate::twin_source`], but with two deliberate differences:
//!
//! - **Registered on BOTH native and web.** `twin://` is filesystem-only
//!   (native); `scenario://` routes reads through [`lunco_storage`], so on web it
//!   reads the same OPFS tree the networking client wrote to
//!   (`lunco_storage::OpfsStorage`). One scheme, uniform on both platforms — the
//!   read backend is the only `#[cfg]` (see [`read_bytes`]).
//! - **Content-addressed, read-only.** A scenario is fetched by CID and verified
//!   on download; this source just serves the cached bytes. It is *not* an
//!   editable Twin (no `twin.toml`, no journal) — the transient "consume a
//!   downloaded scenario" path. Promotion to an editable Twin is a separate step.
//!
//! ## Path shape — `scenario://<id>/<relative>`
//! The reader-facing path (scheme stripped) is `<hex scenario id>/<rel asset>`,
//! joined under `<cache_dir>/scenarios/` — **exactly** where the networking
//! client's download writes each asset (`scenario_asset_path`). So a stage loaded
//! from `scenario://<id>/scenes/main.usda` resolves its co-located refs
//! (`@rover.glb@`) through this same source, on every platform.

use std::path::{Path, PathBuf};

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, ErasedAssetReader, PathStream, Reader,
    VecReader,
};

/// The asset-source scheme for downloaded-scenario assets.
pub const SCENARIO_SCHEME: &str = "scenario";

/// Resolve a reader-facing `scenario://` path (`<id>/<rel>`) to its cache
/// location under `<cache_dir>/scenarios/`. Rejects path traversal — a remote
/// host's asset paths must never escape the scenario cache root (only `Normal`
/// components are kept; `..`, absolute, prefix, or `.` segments fail the read).
fn resolve(path: &Path) -> Option<PathBuf> {
    let mut full = crate::cache_dir().join("scenarios");
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(seg) => full.push(seg),
            _ => return None,
        }
    }
    Some(full)
}

/// Build the `scenario://` [`AssetSourceBuilder`]. Registered via the scheme
/// registry (below), drained by [`crate::register_lunco_asset_sources`] before
/// `AssetPlugin` builds.
pub fn scenario_asset_source() -> AssetSourceBuilder {
    AssetSourceBuilder::new(|| Box::new(ScenarioReader) as Box<dyn ErasedAssetReader>)
}

// Contribute `scenario://` to the asset-scheme registry. lunco-assets owns this
// scheme (it's a cache concern — `<cache_dir>/scenarios/…`, keyed on this crate's
// `cache_dir()`), even though the bytes are fetched by the networking crate.
inventory::submit! {
    crate::asset_sources::AssetSchemeProvider {
        scheme: SCENARIO_SCHEME,
        build: scenario_asset_source,
    }
}

/// Serves cached scenario assets. Zero-sized — the cache root is derived per-read
/// from [`crate::cache_dir`], so no per-instance state.
struct ScenarioReader;

impl AssetReader for ScenarioReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let Some(full) = resolve(path) else {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        match read_bytes(&full).await {
            Some(bytes) => Ok(VecReader::new(bytes)),
            None => Err::<VecReader, _>(AssetReaderError::NotFound(full)),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Scenario assets ship no `.meta` sidecars (the `VecReader` annotation
        // pins the opaque return type; this branch only ever errs).
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        // We only ever serve individual cached files; USD ref resolution loads
        // each file directly. No directory enumeration.
        Ok(false)
    }
}

/// Read cached bytes through the storage backend. The ONLY native/web divergence
/// in this source: native = `FileStorage` (std::fs, via the sync wrapper — this
/// runs on Bevy's async IO pool); web = `OpfsStorage` (async OPFS read).
#[cfg(not(target_arch = "wasm32"))]
async fn read_bytes(full: &Path) -> Option<Vec<u8>> {
    lunco_storage::read_file_sync(full).ok()
}

#[cfg(target_arch = "wasm32")]
async fn read_bytes(full: &Path) -> Option<Vec<u8>> {
    lunco_storage::OpfsStorage::new()
        .read(&lunco_storage::StorageHandle::File(full.to_path_buf()))
        .await
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_maps_under_scenarios_and_rejects_traversal() {
        let base = crate::cache_dir().join("scenarios");
        assert_eq!(
            resolve(Path::new("abcd/scenes/main.usda")),
            Some(base.join("abcd").join("scenes").join("main.usda")),
        );
        assert!(resolve(Path::new("../escape")).is_none());
        assert!(resolve(Path::new("a/../../b")).is_none());
    }
}
