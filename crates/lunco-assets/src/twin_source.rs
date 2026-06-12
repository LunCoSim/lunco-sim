//! `twin://` asset source — reads each open Twin's scene and its **co-located**
//! assets relative to that Twin's root.
//!
//! Lives here next to the other asset-source plumbing ([`crate::cache_dir`],
//! [`crate::lunco_lib_path`], …): this crate is the home for "where assets live
//! and how Bevy reaches them".
//!
//! ## Why it exists
//! An external Twin (a scene in its own repo, outside the engine project) must
//! stay portable. The scene file stores only *relative* refs (`@terrain.glb@`)
//! and library refs (`@lunco://vessels/…@`) — never an absolute path. But Bevy's
//! `AssetServer` only reads from sources registered at app-build time, and on
//! the web there is no filesystem at all, so we can't lean on `std::fs`. So we
//! register ONE `twin://` source backed by a small **registry of Twin roots**.
//!
//! ## Path shape — `twin://<name>/<relative>`
//! The first path segment is the **Twin name** (from its `twin.toml`); the rest
//! is relative to that Twin's root. This keys multiple open Twins independently
//! (no single-mutable-root aliasing) and makes the asset *identity*
//! (`Provenance` source) a stable, machine-independent `twin://moonbase/scene.usda`
//! — identical on every machine, unique per Twin. `twin://` is **internal**:
//! it is never authored into a USD/`twin.toml` file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, ErasedAssetReader, PathStream, Reader,
    VecReader,
};
use bevy::prelude::*;

/// The asset-source scheme for Twin-root-relative assets.
pub const TWIN_SCHEME: &str = "twin";

/// Registry of open Twin roots, keyed by Twin name. Cloneable handle over one
/// shared map: one clone is captured by the registered asset source (read side),
/// another is inserted as a Bevy resource so the Twin-open flow can register
/// roots as folders are opened.
#[derive(Resource, Clone, Default)]
pub struct TwinRoots(Arc<RwLock<HashMap<String, PathBuf>>>);

impl TwinRoots {
    /// Map a Twin `name` to its absolute root folder. Call when a Twin opens,
    /// before loading `twin://<name>/<default_scene>`. Re-registering the same
    /// name (e.g. reopening from a new location) just updates the root.
    pub fn register(&self, name: impl Into<String>, root: impl Into<PathBuf>) {
        if let Ok(mut m) = self.0.write() {
            m.insert(name.into(), root.into());
        }
    }

    fn root_for(&self, name: &str) -> Option<PathBuf> {
        self.0.read().ok().and_then(|m| m.get(name).cloned())
    }
}

/// Build the `twin://` [`AssetSourceBuilder`] over `roots`. Register in each
/// binary BEFORE `AssetPlugin` builds, and insert the same `roots` handle as a
/// resource so the Twin-open flow can register roots.
pub fn twin_asset_source(roots: &TwinRoots) -> AssetSourceBuilder {
    let roots = roots.clone();
    AssetSourceBuilder::new(move || {
        Box::new(TwinReader { roots: roots.clone() }) as Box<dyn ErasedAssetReader>
    })
}

/// `AssetReader` that splits `<name>/<rel>`, looks the Twin root up by name, and
/// reads `<root>/<rel>` into memory (`VecReader`). In-memory reading sidesteps
/// the lifetime dance of returning a borrowed file handle from an `async fn` in
/// the trait, and matches how the wasm file readers already work.
struct TwinReader {
    roots: TwinRoots,
}

impl TwinReader {
    /// Resolve `twin://`-relative `<name>/<rel>` to an absolute filesystem path.
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let mut comps = path.components();
        let name = comps.next()?.as_os_str().to_str()?;
        let root = self.roots.root_for(name)?;
        Some(root.join(comps.as_path()))
    }
}

impl AssetReader for TwinReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let Some(full) = self.resolve(path) else {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        match std::fs::read(&full) {
            Ok(bytes) => Ok(VecReader::new(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AssetReaderError::NotFound(full))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Twin assets ship no `.meta` sidecars. The `VecReader` annotation pins
        // the opaque return type even though this branch only ever errs.
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(self
            .resolve(path)
            .and_then(|full| full.metadata().ok())
            .map(|m| m.is_dir())
            .unwrap_or(false))
    }
}
