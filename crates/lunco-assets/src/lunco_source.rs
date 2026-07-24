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

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSource, AssetSourceBuilder, ErasedAssetReader, PathStream,
    Reader,
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
        Some(rel) => Some(assets_root?.join(rel)),
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

/// Build the `lunco://` [`AssetSourceBuilder`]: `assets/`, then each cache root
/// in [`cache_roots`](crate::cache_roots) order.
pub fn lunco_asset_source(assets_dir: &Path) -> AssetSourceBuilder {
    let mut roots = vec![assets_dir.to_string_lossy().into_owned()];
    roots.extend(
        crate::cache_roots()
            .iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    AssetSourceBuilder::new(move || {
        Box::new(FallbackReader {
            readers: roots
                .iter()
                .map(|r| AssetSource::get_default_reader(r.clone())())
                .collect(),
        }) as Box<dyn ErasedAssetReader>
    })
}

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
struct FallbackReader {
    readers: Vec<Box<dyn ErasedAssetReader>>,
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
        try_both!(self, read, path)
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        try_both!(self, read_meta, path)
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        try_both!(self, read_directory, path)
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
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
