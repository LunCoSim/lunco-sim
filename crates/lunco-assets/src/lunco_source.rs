//! The `lunco://` asset source — the engine asset **library**.
//!
//! `lunco://<rel>` is a *logical* identity: "this asset belongs to the LunCo
//! library". Where the bytes actually sit is a resolution detail, deliberately
//! not part of the address:
//!
//! 1. `assets/<rel>` — git-tracked, authored content
//! 2. `<cache>/<rel>` — externally-fetched binaries declared in `Assets.toml`
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
//! **One resolver, every platform.** Both roots are read through Bevy's own
//! [`AssetSource::get_default_reader`], which yields a file reader natively and
//! an HTTP reader on wasm. So the browser resolves `assets/` then `.cache/`
//! over HTTP exactly as native resolves two directories — the fallback is not a
//! native-only convenience that silently disappears on web.

use std::path::Path;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSource, AssetSourceBuilder, ErasedAssetReader, PathStream,
    Reader,
};

use crate::cache_dir;

/// The asset-source scheme for the engine asset library.
pub const LUNCO_SCHEME: &str = "lunco";

/// Build the `lunco://` [`AssetSourceBuilder`]: `assets/`, then the cache.
pub fn lunco_asset_source(assets_dir: &Path) -> AssetSourceBuilder {
    let assets = assets_dir.to_string_lossy().into_owned();
    let cache = cache_dir().to_string_lossy().into_owned();
    AssetSourceBuilder::new(move || {
        Box::new(FallbackReader {
            primary: AssetSource::get_default_reader(assets.clone())(),
            secondary: AssetSource::get_default_reader(cache.clone())(),
        }) as Box<dyn ErasedAssetReader>
    })
}

/// Reads from `primary`, falling back to `secondary` when the asset is absent.
///
/// Authored content wins: a file committed under `assets/` shadows a cached
/// download of the same relative path, so a repo asset is never silently
/// replaced by whatever an earlier `download` left behind.
///
/// Only [`AssetReaderError::NotFound`] falls through. A genuine I/O failure —
/// permissions, a truncated HTTP response — propagates immediately, because
/// retrying it against the other root would convert a real error into a
/// confusing "not found" and hide the actual cause.
struct FallbackReader {
    primary: Box<dyn ErasedAssetReader>,
    secondary: Box<dyn ErasedAssetReader>,
}

/// Run `primary`, then `secondary` iff the first reported `NotFound`.
macro_rules! try_both {
    ($self:ident, $method:ident, $path:expr) => {
        match $self.primary.$method($path).await {
            Err(AssetReaderError::NotFound(_)) => $self.secondary.$method($path).await,
            other => other,
        }
    };
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
        match self.primary.is_directory(path).await {
            Ok(false) | Err(AssetReaderError::NotFound(_)) => {
                self.secondary.is_directory(path).await
            }
            other => other,
        }
    }
}
