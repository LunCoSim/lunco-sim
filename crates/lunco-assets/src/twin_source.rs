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

/// The key an overlay is stored under — the reader-facing relative path
/// `<name>/<rel>`, matching what [`AssetReader::read`] receives once the
/// `twin://` scheme is stripped.
fn overlay_key(name: &str, rel: &str) -> PathBuf {
    Path::new(name).join(rel)
}

/// Registry of open Twin roots, keyed by Twin name. Cloneable handle over two
/// shared maps: one clone is captured by the registered asset source (read side),
/// another is inserted as a Bevy resource so the Twin-open flow can register
/// roots as folders are opened.
///
/// The second map — [`overlays`](TwinRoots::set_overlay) — lets a caller serve
/// **in-memory bytes** for a specific `twin://<name>/<rel>` path instead of the
/// on-disk file. This is the E1b seam: lunco-usd registers a scene document's
/// *composed* (`base ⊕ runtime`) source as the overlay, so the async `UsdLoader`
/// composes the live world from the editable document — anchored at the same
/// `twin://` identity, so co-located refs (terrain `.glb`) still resolve, on
/// every platform the twin source supports.
#[derive(Resource, Clone, Default)]
pub struct TwinRoots {
    /// Twin name → absolute root folder.
    roots: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// `twin://`-relative path (`<name>/<rel>`) → in-memory bytes that shadow
    /// the on-disk file for that exact path.
    overlays: Arc<RwLock<HashMap<PathBuf, Arc<Vec<u8>>>>>,
}

impl TwinRoots {
    /// Map a Twin `name` to its absolute root folder, returning the name
    /// actually assigned — **callers must use the returned name**, not the one
    /// they passed.
    ///
    /// Call when a Twin opens, before loading `twin://<name>/<default_scene>`.
    ///
    /// The name is the `twin://` authority, so it must stay human-readable and
    /// machine-independent (it is the stable provenance identity — see
    /// `docs/architecture/21-domain-usd.md`). That rules out keying by
    /// canonical path. But names are *not* unique: the name comes from
    /// `twin.toml`, falling back to the folder's basename, so two unrelated
    /// folders can both be `scenes`. Blindly inserting silently repointed the
    /// first Twin's root, breaking every `twin://first/…` read already in
    /// flight, with no diagnostic.
    ///
    /// So: re-registering the *same* root under a name is idempotent (a reopen),
    /// while a *different* root gets the next free `name-2`, `name-3`, … .
    #[must_use = "use the RETURNED name to build `twin://` URIs — the requested \
                  name may already belong to a different root"]
    pub fn register(&self, name: impl Into<String>, root: impl Into<PathBuf>) -> String {
        let requested = name.into();
        let root = root.into();
        let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let Ok(mut m) = self.roots.write() else {
            return requested;
        };
        let same_root = |existing: &PathBuf| {
            std::fs::canonicalize(existing).unwrap_or_else(|_| existing.clone()) == canonical
        };
        let mut candidate = requested.clone();
        let mut n = 1u32;
        loop {
            match m.get(&candidate) {
                // Free, or already this exact root (reopen) — take it.
                None => break,
                Some(existing) if same_root(existing) => break,
                // Taken by a different folder — try the next suffix.
                Some(_) => {
                    n += 1;
                    candidate = format!("{requested}-{n}");
                }
            }
        }
        if candidate != requested {
            warn!(
                "[twin-roots] name `{requested}` is already bound to a different folder — \
                 registering `{}` as `{candidate}`",
                root.display()
            );
        }
        m.insert(candidate.clone(), root);
        candidate
    }

    /// Serve `bytes` in place of the on-disk file at `twin://<name>/<rel>`. The
    /// key matches the path the `AssetReader` receives (scheme stripped), so a
    /// subsequent `AssetServer` load/reload of `twin://<name>/<rel>` reads these
    /// bytes. Used by E1b to project a document's composed source into the live
    /// world; pass the same `(name, rel)` to [`clear_overlay`](Self::clear_overlay)
    /// to fall back to disk.
    pub fn set_overlay(&self, name: &str, rel: &str, bytes: Arc<Vec<u8>>) {
        if let Ok(mut m) = self.overlays.write() {
            m.insert(overlay_key(name, rel), bytes);
        }
    }

    /// Drop the in-memory overlay for `twin://<name>/<rel>` so reads fall back
    /// to the on-disk file again.
    pub fn clear_overlay(&self, name: &str, rel: &str) {
        if let Ok(mut m) = self.overlays.write() {
            m.remove(&overlay_key(name, rel));
        }
    }

    /// Overlay bytes registered for the reader-facing relative `path`
    /// (`<name>/<rel>`), if any.
    fn overlay_for(&self, path: &Path) -> Option<Arc<Vec<u8>>> {
        self.overlays.read().ok().and_then(|m| m.get(path).cloned())
    }

    fn root_for(&self, name: &str) -> Option<PathBuf> {
        self.roots.read().ok().and_then(|m| m.get(name).cloned())
    }

    /// Names of all currently-open Twins, sorted (deterministic order — the
    /// map's own iteration order isn't).
    pub fn names(&self) -> Vec<String> {
        self.roots
            .read()
            .ok()
            .map(|m| {
                let mut v: Vec<String> = m.keys().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    /// Absolute root folder for an open Twin by name.
    pub fn root_of(&self, name: &str) -> Option<PathBuf> {
        self.root_for(name)
    }

    /// The "primary" open Twin as `(name, root)` — the alphabetically-first
    /// registered Twin, used as the default destination for newly created or
    /// imported assets when the caller doesn't name a Twin. `None` if no Twin
    /// is open.
    pub fn primary(&self) -> Option<(String, PathBuf)> {
        self.names()
            .into_iter()
            .next()
            .and_then(|n| self.root_for(&n).map(|r| (n, r)))
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
        let first = comps.next()?;
        let first_str = first.as_os_str().to_str()?;
        if first_str == ".." {
            // Relative path escaped the Twin root!
            // Resolve relative to the parent of the primary Twin root.
            let (_, primary_root) = self.roots.primary()?;
            let parent = primary_root.parent()?;
            Some(parent.join(path))
        } else {
            let root = self.roots.root_for(first_str)?;
            Some(root.join(comps.as_path()))
        }
    }
}

impl AssetReader for TwinReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // In-memory overlay wins over the on-disk file (E1b: a scene document's
        // composed source projected into the live world). Keyed by the exact
        // reader-facing `<name>/<rel>` path.
        if let Some(bytes) = self.roots.overlay_for(path) {
            return Ok(VecReader::new((*bytes).clone()));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay must be keyed identically to the path the `AssetReader`
    /// receives for `twin://<name>/<rel>` (scheme stripped) — otherwise an
    /// `AssetServer` load would miss it and read the on-disk file.
    #[test]
    fn overlay_keyed_by_reader_facing_path() {
        let roots = TwinRoots::default();
        let bytes = Arc::new(b"#usda 1.0\n".to_vec());
        roots.set_overlay("moonbase", "scenes/sandbox.usda", bytes.clone());

        // The reader receives `moonbase/scenes/sandbox.usda` (scheme stripped).
        assert_eq!(
            roots.overlay_for(Path::new("moonbase/scenes/sandbox.usda")).as_deref(),
            Some(&*bytes),
            "overlay hit for the exact reader-facing path"
        );
        assert!(
            roots.overlay_for(Path::new("moonbase/other.usda")).is_none(),
            "no overlay for an unrelated path"
        );

        roots.clear_overlay("moonbase", "scenes/sandbox.usda");
        assert!(
            roots.overlay_for(Path::new("moonbase/scenes/sandbox.usda")).is_none(),
            "cleared overlay falls back to disk"
        );
    }

    /// Two unrelated folders can carry the same name (`twin.toml` name, or a
    /// basename like `scenes`). Registering the second must NOT repoint the
    /// first — that silently broke every `twin://first/…` read already in
    /// flight, with no diagnostic.
    #[test]
    fn same_name_different_root_does_not_repoint() {
        let roots = TwinRoots::default();

        let a = roots.register("scenes", "/tmp/project-a/scenes");
        let b = roots.register("scenes", "/tmp/project-b/scenes");

        assert_eq!(a, "scenes");
        assert_ne!(b, a, "second root must not take the first root's name");
        assert_eq!(
            roots.root_of(&a),
            Some(PathBuf::from("/tmp/project-a/scenes")),
            "first Twin still resolves to its own folder"
        );
        assert_eq!(
            roots.root_of(&b),
            Some(PathBuf::from("/tmp/project-b/scenes")),
            "second Twin resolves to its own folder under the assigned name"
        );
    }

    /// Reopening the SAME folder is idempotent — it must reuse the name rather
    /// than accumulating `scenes-2`, `scenes-3`, … on every reopen.
    #[test]
    fn reregistering_same_root_reuses_the_name() {
        let roots = TwinRoots::default();

        let first = roots.register("moonbase", "/tmp/moonbase");
        let again = roots.register("moonbase", "/tmp/moonbase");

        assert_eq!(first, "moonbase");
        assert_eq!(again, first, "reopen must reuse the existing name");
        assert_eq!(roots.names().len(), 1, "no duplicate registration");
    }
}
