//! The `lunco://` asset source — the engine asset **library**.
//!
//! `lunco://<rel>` is a *logical* identity: "this asset belongs to the LunCo
//! library". Where the bytes actually sit is a resolution detail, deliberately
//! not part of the address:
//!
//! 1. `assets/<rel>` — git-tracked, authored content
//! 2. `assets/.cache/<rel>` — the PACKED cache: binaries shipped inside the
//!    distribution, so a packaged build carries its own payload
//! 3. `<cache>/<rel>` — the shared machine-wide pool, filled by the downloader
//!
//! This replaced the old `lunco-lib://` scheme. That scheme addressed the cache
//! *directly*, so a `.usda` shipped in the repo asserted "this asset lives in my
//! download cache" — a machine-local fact baked into authored content, which
//! resolved only inside our pipeline and left third-party USD tools with
//! nothing. Large binaries still stay out of git; they are *resolved* into the
//! library rather than *addressed* in the cache, so nothing needs gitignoring
//! and no authored file mentions where a download landed.
//!
//! See `docs/architecture/56-asset-resolution-and-cache.md`.
//!
//! **One resolver, every platform.** Every root is read through Bevy's own
//! [`AssetSource::get_default_reader`], which yields a file reader natively and
//! an HTTP reader on wasm. So the browser resolves the same chain over HTTP as
//! native resolves over directories — the fallback is not a native-only
//! convenience that silently disappears on web.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSource, AssetSourceBuilder, AssetWatcher,
    ErasedAssetReader, PathStream, Reader,
};

/// The asset-source scheme for the engine asset library — the name it is
/// registered under, both as a Bevy `AssetSource` and in the
/// [`SchemeRegistry`](crate::scheme_registry::SchemeRegistry).
pub const LUNCO_SCHEME: &str = "lunco";

/// The library-relative path of a `lunco://<rel>` reference, or `None` for a bare
/// or differently-schemed one. Unlike [`crate::engine_asset_rel`] (which treats a
/// bare path as already-relative), this distinguishes "explicitly addressed to
/// the engine library" — what a caller re-rooting an id back onto disk needs.
pub fn parse_lunco_uri(uri: &str) -> Option<&str> {
    let (scheme, rel) = crate::asset_path::split_scheme(uri)?;
    (scheme == LUNCO_SCHEME).then_some(rel)
}

/// The directory name the shipped asset library lives under (`assets`). The
/// `lunco://` source is anchored on it, so code walking a path's ancestors to
/// find that root must ask here rather than spell the literal again.
pub const ASSETS_DIR_NAME: &str = "assets";

/// The shipped-asset root (`…/assets`) an on-disk file lives under, if any —
/// the directory `lunco://` is anchored at *for that file*.
///
/// Distinct from [`crate::assets_dir_abs`], which anchors on the process CWD:
/// this answers the question for a file that may live outside the running
/// project (a tool composing a `.usda` by absolute path), so it walks ancestors
/// instead of assuming the CWD is the project.
pub fn shipped_asset_root(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|a| a.file_name() == Some(std::ffi::OsStr::new(ASSETS_DIR_NAME)))
}

/// Map an asset id back to the file holding its bytes: `lunco://<rel>` resolves
/// against `assets_root`, anything else is treated as a filesystem path.
///
/// `None` when the id names the shipped library but no library root was found —
/// the caller composed a file that lives outside any `assets/` tree, so a
/// `lunco://` reference in it cannot be reached.
///
/// A source-relative id (one whose leading `/` was stripped when it was
/// canonicalized) is re-rooted, since it has to become absolute to be readable
/// again. A drive-qualified Windows path is already absolute and passes through.
pub fn id_to_disk_path(id: &str, assets_root: Option<&Path>) -> Option<PathBuf> {
    match parse_lunco_uri(id) {
        Some(rel) if crate::asset_path::is_safe_relative_path(rel) => Some(assets_root?.join(rel)),
        Some(_) => None,
        None => {
            let p = PathBuf::from(id);
            Some(if p.is_absolute() {
                p
            } else {
                Path::new("/").join(id)
            })
        }
    }
}

/// Read bytes for a canonical asset identity through the native asset-location
/// policy. USD composition deliberately calls this instead of touching the
/// filesystem: asset-root selection and diagnostic paths stay owned here.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes(id: &str, assets_root: Option<&Path>) -> std::io::Result<Vec<u8>> {
    let path = id_to_disk_path(id, assets_root).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("asset `{id}` has no resolvable native root"),
        )
    })?;
    std::fs::read(path)
}

/// Read a canonical asset identity when the composing document belongs to an
/// open Twin.  The synchronous USD composer cannot call Bevy's async reader,
/// so it uses this asset-owned equivalent of the registered `twin://` source.
/// Library assets keep the ordinary `assets_root` policy; Twin assets are
/// resolved inside the explicitly supplied Twin root and its cache.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes_with_twin_root(
    id: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> std::io::Result<Vec<u8>> {
    if let Some((_name, rel)) = crate::parse_twin_uri(id) {
        let root = twin_root.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Twin asset `{id}` has no composing Twin root"),
            )
        })?;
        if !crate::asset_path::is_safe_relative_path(rel) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Twin asset `{id}` escapes its root"),
            ));
        }
        let relative = PathBuf::from(rel);
        let authored = root.join(&relative);
        if authored.is_file() {
            return std::fs::read(authored);
        }
        let cached = crate::twin_cache_dir(root).join(relative);
        return std::fs::read(cached);
    }
    read_asset_bytes(id, assets_root)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn synchronous_twin_reads_share_the_canonical_traversal_guard() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        std::fs::write(root.path().join("lesson.rhai"), "40 + 2").expect("lesson");

        let id = "twin://example/lesson.rhai";
        assert_eq!(
            read_asset_bytes_with_twin_root(id, None, Some(root.path())).expect("authored file"),
            b"40 + 2"
        );
        for id in [
            "twin://example/../outside.rhai",
            "twin://example/a/../../outside.rhai",
            r"twin://example/..\outside.rhai",
        ] {
            let error = read_asset_bytes_with_twin_root(id, None, Some(root.path()))
                .expect_err("unsafe Twin path must be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn library_ids_cannot_escape_the_asset_root() {
        let root = tempfile::tempdir().expect("temporary asset root");
        assert_eq!(
            id_to_disk_path("lunco://terrain/moon.usda", Some(root.path())),
            Some(root.path().join("terrain/moon.usda"))
        );
        for id in [
            "lunco://../outside.usda",
            "lunco://terrain/../../outside.usda",
            r"lunco://terrain\outside.usda",
        ] {
            assert!(
                id_to_disk_path(id, Some(root.path())).is_none(),
                "unsafe library id must be rejected: {id}"
            );
        }
    }
}

/// Native read for a caller-selected root document. Kept in `lunco-assets` so
/// USD consumers never perform their own filesystem access.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Read authored UTF-8 text through the asset boundary.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read_asset_file_bytes(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Build the `lunco://` [`AssetSourceBuilder`]: `assets/`, then each cache root
/// in [`cache_roots`](crate::cache_roots) order.
///
/// Only the authored `assets/` tree is watched. Cache roots are read-only
/// materialized artifacts and can contain tens of thousands of directories;
/// recursively registering an OS watch for them is both unnecessary for live
/// authoring and can exhaust the process' watch quota before the app starts.
pub fn lunco_asset_source(assets_dir: &Path) -> AssetSourceBuilder {
    let watch_root = assets_dir.to_string_lossy().into_owned();
    let mut roots = vec![assets_dir.to_string_lossy().into_owned()];
    roots.extend(
        crate::cache_roots()
            .iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    let reader_roots = roots.clone();
    AssetSourceBuilder::new(move || {
        Box::new(FallbackReader {
            readers: reader_roots
                .iter()
                .map(|r| AssetSource::get_default_reader(r.clone())())
                .collect(),
            roots: reader_roots.clone(),
        }) as Box<dyn ErasedAssetReader>
    })
    .with_watcher(move |sender| {
        if !Path::new(&watch_root).exists() {
            return None;
        }
        let mut build =
            AssetSource::get_default_watcher(watch_root.clone(), Duration::from_millis(300));
        build(sender)
            .map(|watcher| Box::new(FallbackWatcher { _watcher: watcher }) as Box<dyn AssetWatcher>)
    })
}

/// Keeps the authored-tree watcher backing the fallback reader alive.
///
/// The reader's existing priority still decides which bytes win, so a cache
/// artifact can never override an authored asset. Cache population is handled
/// by the asset/download boundary; it is not an authoring edit stream.
struct FallbackWatcher {
    _watcher: Box<dyn AssetWatcher>,
}

impl AssetWatcher for FallbackWatcher {}

/// Reads each root in turn, moving on only when the asset is absent there.
///
/// Order is priority: authored content wins over the packed cache, which wins
/// over the shared pool. So a file committed under `assets/` is never silently
/// replaced by whatever a download left behind, and a distribution's own
/// payload is never shadowed by a stale copy in the machine-wide cache.
///
/// Only [`AssetReaderError::NotFound`] falls through. A genuine I/O failure —
/// permissions, a truncated HTTP response — propagates immediately, because
/// retrying it against the next root would convert a real error into a
/// confusing "not found" and hide the actual cause. The LAST root's error is
/// the one returned, so a miss reports the deepest place we looked.
///
/// That last part is a trap for whoever reads the log, which is why [`read`]
/// also names every root. A miss on `lunco://components/cameras/lunar_surface_camera.usda`
/// surfaced as `Path not found: C:\Users\…\AppData\Local\lunco\environment/…`,
/// and a bug report reasonably concluded that `lunco://` resolved *into the
/// AppData cache and never into the install's own `assets/`* — the exact
/// opposite of the resolution order, which tries `assets/` FIRST. The proposed
/// remedy ("fall back to `<install>/assets` on a cache miss") was already the
/// behaviour, in reverse. One root named out of three read as the only root
/// tried.
///
/// [`read`]: AssetReader::read
struct FallbackReader {
    readers: Vec<Box<dyn ErasedAssetReader>>,
    /// Parallel to `readers`, kept solely so a miss can say where it looked.
    roots: Vec<String>,
}

/// Try each root in order; the first non-`NotFound` answer wins.
macro_rules! try_both {
    ($self:ident, $method:ident, $path:expr) => {{
        // `readers` is non-empty by construction (`assets/` is always first),
        // so the loop always assigns before the unwrap.
        let mut last = None;
        for reader in &$self.readers {
            match reader.$method($path).await {
                Err(AssetReaderError::NotFound(p)) => {
                    last = Some(Err(AssetReaderError::NotFound(p)))
                }
                other => return other,
            }
        }
        last.unwrap()
    }};
}

impl AssetReader for FallbackReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        let result = try_both!(self, read, path);
        if matches!(result, Err(AssetReaderError::NotFound(_))) {
            // Only `read`. Bevy probes for a sibling `.meta` on EVERY asset and
            // almost never finds one, so warning from `read_meta` would bury
            // this line in noise from the ordinary case.
            bevy::log::warn!(
                "[lunco://] `{}` not found in any library root. Looked in order:\n{}",
                path.display(),
                self.roots
                    .iter()
                    .enumerate()
                    .map(|(i, root)| format!("  {}. {root}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        result
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        try_both!(self, read_meta, path)
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        try_both!(self, read_directory, path)
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Ok(false);
        }
        // `is_directory` answers false rather than erroring for a missing path,
        // so `NotFound` is not the signal here — a plain `false` is.
        let mut last = Ok(false);
        for reader in &self.readers {
            match reader.is_directory(path).await {
                Ok(false) => last = Ok(false),
                Err(AssetReaderError::NotFound(p)) => last = Err(AssetReaderError::NotFound(p)),
                other => return other,
            }
        }
        last
    }
}
