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
//! register ONE `twin://` source backed by a small **registry of Twin roots**,
//! reading through [`lunco_storage`] so the SAME scheme serves native and web.
//!
//! A root is an open Twin's directory OR a downloaded scenario's cache directory.
//! One scheme for both is what keeps a scene's asset path identical on every peer,
//! and therefore its `Provenance::Content`-derived `GlobalEntityId` identical too.
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

/// The asset-source scheme for Twin-root-relative assets — the name it is
/// registered under, both as a Bevy `AssetSource` and in the
/// [`SchemeRegistry`](crate::scheme_registry::SchemeRegistry).
pub const TWIN_SCHEME: &str = "twin";

/// The `twin://<name>/<rel>` URI naming `rel` inside the Twin `name` — the ONE
/// place the scheme string is spelled into an address. Callers that hand-rolled
/// `format!("twin://{name}/{rel}")` duplicated resolution knowledge this crate
/// owns; a scheme rename must not require editing five crates.
///
/// `rel` is normalised to forward slashes (a URI is not a `Path`) and stripped of
/// a leading `/`, so a Windows-built relative path still names the same asset on
/// every peer — the identity has to be byte-identical across the wire.
pub fn twin_uri(name: impl AsRef<str>, rel: impl AsRef<Path>) -> String {
    let rel = crate::asset_path::slashed(rel);
    crate::asset_path::uri(
        TWIN_SCHEME,
        &format!("{}/{}", name.as_ref(), rel.trim_start_matches('/')),
    )
}

/// Split a `twin://<name>/<rel>` URI into its parts, or `None` when it carries a
/// different scheme (or no scheme at all). The parsing inverse of [`twin_uri`].
pub fn parse_twin_uri(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = crate::asset_path::split_scheme(uri)?;
    split_twin_rel((scheme == TWIN_SCHEME).then_some(rest)?)
}

/// Split the scheme-stripped remainder of a `twin://` address — the
/// `<name>/<rel>` form an `AssetReader` or scheme handler receives — into the
/// Twin name and the Twin-relative path. [`parse_twin_uri`] for callers that
/// hold the full URI.
pub fn split_twin_rel(rest: &str) -> Option<(&str, &str)> {
    rest.split_once(|separator| separator == '/' || separator == '\\')
}

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

fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Resolve a Twin-relative path with the same authored-first, cache-second
/// policy used by the `twin://` reader. The AssetServer, dataset registry, and
/// native runtime consumers must agree on which roots a logical Twin path names.
fn resolve_twin_relative_path(root: &Path, relative: &Path) -> Option<PathBuf> {
    if !crate::asset_path::is_safe_relative_components(relative) {
        return None;
    }
    let authored = root.join(relative);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = crate::existing_path_within_root(root, relative) {
        return Some(path);
    }
    #[cfg(target_arch = "wasm32")]
    if authored.exists() {
        return Some(authored.clone());
    }
    if authored.exists() {
        // A present entry that failed canonical containment is either an
        // escaping symlink or an entry the native filesystem refused to
        // canonicalize. Do not hand it to the storage backend.
        return None;
    }

    let cache_root = crate::twin_cache_dir(root);
    let cached = cache_root.join(relative);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = crate::existing_path_within_root(&cache_root, relative) {
        return Some(path);
    }
    #[cfg(target_arch = "wasm32")]
    if cached.exists() {
        return Some(cached.clone());
    }
    if cached.exists() {
        // The cache entry failed containment; never turn a remote Twin
        // reference into an arbitrary host filesystem read.
        return None;
    }
    // Preserve the reader's useful missing-file diagnostic: authored is the
    // logical location a Twin-relative reference names.
    Some(authored)
}

pub(crate) fn resolve_twin_relative_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    resolve_twin_relative_path(root, relative).filter(|path| path.is_file())
}

pub(crate) fn resolve_twin_relative_directory(root: &Path, relative: &Path) -> Option<PathBuf> {
    resolve_twin_relative_path(root, relative).filter(|path| path.is_dir())
}

impl TwinRoots {
    /// Register the asset authority for an opened workspace Twin.
    ///
    /// The workspace event remains the normal lifecycle trigger, but startup
    /// composition can admit a Twin and immediately request its default scene
    /// in the same observer dispatch. Exposing this operation on the asset
    /// owner lets that composition root establish the `twin://` authority
    /// before any domain observer resolves the scene; repeated registration is
    /// idempotent for the same root.
    pub fn register_twin(&self, twin: &lunco_twin::Twin) -> String {
        let name = twin
            .manifest
            .as_ref()
            .map(|manifest| manifest.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                twin.root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "twin".to_string());
        self.register(name, twin.root.clone())
    }

    fn clear_overlays_for_names(&self, names: &[String]) {
        if let Ok(mut overlays) = self.overlays.write() {
            overlays.retain(|path, _| {
                path.components()
                    .next()
                    .and_then(|component| component.as_os_str().to_str())
                    .is_none_or(|name| !names.iter().any(|removed| removed == name))
            });
        }
    }

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
        let canonical = canonical_root(&root);
        let Ok(mut m) = self.roots.write() else {
            return requested;
        };
        let same_root = |existing: &PathBuf| canonical_root(existing) == canonical;
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
        if !crate::asset_path::is_safe_relative_path(name)
            || !crate::asset_path::is_safe_relative_path(rel)
        {
            return;
        }
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
        let path = PathBuf::from(crate::asset_path::slashed(path));
        self.overlays
            .read()
            .ok()
            .and_then(|m| m.get(&path).cloned())
    }

    /// Absolute root folder of an open Twin, by `twin://` authority. Public
    /// because a Twin's own `Assets.toml` (scanned on open by
    /// [`crate::datasets`]) is addressed by filesystem path, not by URI.
    pub fn root_for(&self, name: &str) -> Option<PathBuf> {
        self.roots.read().ok().and_then(|m| m.get(name).cloned())
    }

    /// Return the authority assigned to an open Twin root.
    ///
    /// Names can be disambiguated when two open folders share the same
    /// authored/folder name, so consumers must resolve the assigned authority
    /// instead of reconstructing it from the manifest again.
    pub fn name_for_root(&self, root: impl AsRef<Path>) -> Option<String> {
        let target = canonical_root(root.as_ref());
        self.roots.read().ok().and_then(|roots| {
            roots
                .iter()
                .find(|(_, existing)| canonical_root(existing) == target)
                .map(|(name, _)| name.clone())
        })
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

    /// Resolve a Twin-relative file using the same authored-first, cache-second
    /// policy as the `twin://` AssetReader. Native consumers that need a concrete
    /// filesystem path must use this boundary rather than joining a Twin root
    /// themselves, because downloaded Twin assets live in `.cache`.
    pub fn resolve_file(&self, name: &str, relative: &Path) -> Option<PathBuf> {
        let root = self.root_for(name)?;
        resolve_twin_relative_file(&root, relative)
    }

    /// Resolve a Twin-relative directory using the same authored-first,
    /// cache-second policy as the `twin://` AssetReader.
    ///
    /// Processed datasets such as DEM sites deliver a directory containing
    /// their runtime products. Directory consumers use this boundary instead
    /// of reconstructing the Twin cache path themselves.
    pub fn resolve_directory(&self, name: &str, relative: &Path) -> Option<PathBuf> {
        let root = self.root_for(name)?;
        resolve_twin_relative_directory(&root, relative)
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

    /// Retire every registered Twin authority backed by `root`, including its
    /// composed-document overlays. A closed workspace Twin must not remain a
    /// valid source for a late asset request from the outgoing scene.
    pub fn unregister_root(&self, root: impl AsRef<Path>) {
        let target = canonical_root(root.as_ref());
        let removed = self
            .roots
            .write()
            .ok()
            .map(|mut roots| {
                let mut names = Vec::new();
                roots.retain(|name, existing| {
                    let same = canonical_root(existing) == target;
                    if same {
                        names.push(name.clone());
                    }
                    !same
                });
                names
            })
            .unwrap_or_default();
        if removed.is_empty() {
            return;
        }
        self.clear_overlays_for_names(&removed);
    }

    /// Retire one synthetic or user-session Twin authority by its exact name,
    /// including its composed-document overlays. This is distinct from
    /// [`unregister_root`](Self::unregister_root): several authorities may
    /// intentionally point at the same directory, so a document view must not
    /// tear down an unrelated Twin merely because their roots match.
    pub fn unregister_name(&self, name: &str) {
        let removed = self
            .roots
            .write()
            .ok()
            .and_then(|mut roots| roots.remove(name).map(|_| name.to_string()));
        if removed.is_none() {
            return;
        }
        self.clear_overlays_for_names(&[name.to_string()]);
    }
}

/// Read a Twin-root file through the storage backend. The ONLY native/web
/// divergence in this source: native = `FileStorage` (std::fs, via the sync
/// wrapper — this runs on Bevy's async IO pool); web = `OpfsStorage` (async OPFS
/// read), which is the same tree the networking client writes a downloaded
/// scenario into. Going through storage is what lets `twin://` serve a downloaded
/// scenario on the web, where there is no filesystem.
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

/// Build the `twin://` [`AssetSourceBuilder`] over `roots`. Register in each
/// binary BEFORE `AssetPlugin` builds, and insert the same `roots` handle as a
/// resource so the Twin-open flow can register roots.
pub fn twin_asset_source(roots: &TwinRoots) -> AssetSourceBuilder {
    let roots = roots.clone();
    AssetSourceBuilder::new(move || {
        Box::new(TwinReader {
            roots: roots.clone(),
        }) as Box<dyn ErasedAssetReader>
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
    ///
    /// Rejects path traversal: only `Normal` components are joined, so a scene can
    /// never reach outside its Twin root. That guard is not optional — a Twin root
    /// may be a **downloaded scenario's cache directory**, whose relative paths were
    /// authored by a remote host, and escaping it would let a peer read arbitrary
    /// local files. Shipped assets are addressed by scheme (`@lunco://…@`), so no
    /// authored ref needs to climb out (verified across the shipped tree and the
    /// twins: zero `@../…@` refs).
    /// A Twin's DOWNLOADED assets live in its own `.cache/` (see
    /// [`crate::twin_cache_dir`]), so a reference authored against the Twin
    /// (`@terrain/apollo15/dtm.tif@`) resolves whether the file is committed in
    /// the Twin or was fetched from that Twin's `Assets.toml`. Authored files
    /// win: the cache is a materialisation of a declaration, never an override
    /// of something the author checked in.
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let mut comps = path.components();
        let name = comps.next()?.as_os_str().to_str()?;
        let mut rel = PathBuf::new();
        for comp in comps {
            rel.push(comp.as_os_str());
        }
        self.roots.resolve_file(name, &rel)
    }
}

impl AssetReader for TwinReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        }
        // In-memory overlay wins over the on-disk file (E1b: a scene document's
        // composed source projected into the live world). Keyed by the exact
        // reader-facing `<name>/<rel>` path.
        if let Some(bytes) = self.roots.overlay_for(path) {
            return Ok(VecReader::new((*bytes).clone()));
        }
        let Some(full) = self.resolve(path) else {
            return Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()));
        };
        match read_bytes(&full).await {
            Some(bytes) => Ok(VecReader::new(bytes)),
            None => Err::<VecReader, _>(AssetReaderError::NotFound(full)),
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

    #[test]
    fn twin_uri_normalizes_windows_relative_paths() {
        assert_eq!(
            twin_uri(
                "Summer Space School",
                Path::new(r"sim\scenes\traverse.usda")
            ),
            "twin://Summer Space School/sim/scenes/traverse.usda"
        );
    }

    #[test]
    fn parses_a_windows_authored_twin_uri() {
        assert_eq!(
            parse_twin_uri(r"twin://SummerSpaceSchool\sim\scenes\traverse.usda"),
            Some(("SummerSpaceSchool", r"sim\scenes\traverse.usda"))
        );
    }

    /// The overlay must be keyed identically to the path the `AssetReader`
    /// receives for `twin://<name>/<rel>` (scheme stripped) — otherwise an
    /// `AssetServer` load would miss it and read the on-disk file.
    #[test]
    fn overlay_keyed_by_reader_facing_path() {
        let roots = TwinRoots::default();
        let bytes = Arc::new(b"#usda 1.0\n".to_vec());
        roots.set_overlay("moonbase", "scenes/luncosim.usda", bytes.clone());

        // The reader receives `moonbase/scenes/luncosim.usda` (scheme stripped).
        assert_eq!(
            roots
                .overlay_for(Path::new("moonbase/scenes/luncosim.usda"))
                .as_deref(),
            Some(&*bytes),
            "overlay hit for the exact reader-facing path"
        );
        assert_eq!(
            roots
                .overlay_for(Path::new(r"moonbase\scenes\luncosim.usda"))
                .as_deref(),
            Some(&*bytes),
            "a Windows reader-facing path finds the slash-normalized overlay"
        );
        assert!(
            roots
                .overlay_for(Path::new("moonbase/other.usda"))
                .is_none(),
            "no overlay for an unrelated path"
        );

        roots.clear_overlay("moonbase", "scenes/luncosim.usda");
        assert!(
            roots
                .overlay_for(Path::new("moonbase/scenes/luncosim.usda"))
                .is_none(),
            "cleared overlay falls back to disk"
        );
    }

    #[test]
    fn unsafe_overlay_paths_are_not_stored_or_read() {
        let roots = TwinRoots::default();
        roots.set_overlay("moonbase", "../outside.usda", Arc::new(b"secret".to_vec()));
        roots.set_overlay("../outside", "scene.usda", Arc::new(b"secret".to_vec()));

        assert!(roots
            .overlay_for(Path::new("moonbase/../outside.usda"))
            .is_none());
        assert!(roots
            .overlay_for(Path::new("../outside/scene.usda"))
            .is_none());
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

    #[test]
    fn resolve_file_finds_downloaded_twin_assets_without_exposing_cache_in_authored_paths() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let cached = crate::twin_cache_dir(twin.path()).join("terrain/luna2");
        std::fs::create_dir_all(cached.parent().expect("cached parent")).expect("cache directory");
        std::fs::write(&cached, b"cached terrain").expect("cached asset");
        let roots = TwinRoots::default();
        let name = roots.register("luna2", twin.path());

        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Some(cached.clone()),
            "logical Twin paths must resolve downloaded assets through the cache"
        );

        let authored = twin.path().join("terrain/luna2");
        std::fs::create_dir_all(authored.parent().expect("authored parent"))
            .expect("authored directory");
        std::fs::write(&authored, b"authored terrain").expect("authored asset");
        assert_eq!(
            roots.resolve_file(&name, Path::new("terrain/luna2")),
            Some(authored),
            "authored Twin files take precedence over materialized cache files"
        );
    }

    #[test]
    fn resolve_directory_finds_processed_twin_assets_without_reconstructing_cache_paths() {
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let cached = crate::twin_cache_dir(twin.path()).join("terrain/luna2");
        std::fs::create_dir_all(cached.join("materials/textures"))
            .expect("cached processed directory");
        std::fs::write(
            cached.join("materials/textures/heightmap.tif"),
            b"processed terrain",
        )
        .expect("cached processed asset");
        let roots = TwinRoots::default();
        let name = roots.register("luna2", twin.path());

        assert_eq!(
            roots.resolve_directory(&name, Path::new("terrain/luna2")),
            Some(cached.clone()),
            "processed Twin directories must resolve through the asset boundary"
        );
    }

    #[test]
    fn unregistering_a_root_removes_its_authority_and_overlays() {
        let roots = TwinRoots::default();
        let name = roots.register("moonbase", "/tmp/moonbase");
        roots.set_overlay(&name, "scene.usda", Arc::new(b"#usda 1.0\n".to_vec()));

        roots.unregister_root("/tmp/moonbase");

        assert!(roots.names().is_empty());
        assert!(roots.root_of(&name).is_none());
        assert!(roots
            .overlay_for(Path::new("moonbase/scene.usda"))
            .is_none());
    }

    #[test]
    fn unregistering_a_name_preserves_another_authority_on_the_same_root() {
        let roots = TwinRoots::default();
        let twin = roots.register("moonbase", "/tmp/moonbase");
        let session = roots.register("__viewport_1", "/tmp/moonbase");
        roots.set_overlay(&twin, "scene.usda", Arc::new(b"twin".to_vec()));
        roots.set_overlay(&session, "scene.usda", Arc::new(b"session".to_vec()));

        roots.unregister_name(&session);

        assert!(roots.root_of(&twin).is_some());
        assert!(roots.root_of(&session).is_none());
        assert!(roots
            .overlay_for(Path::new("moonbase/scene.usda"))
            .is_some());
        assert!(roots
            .overlay_for(Path::new("__viewport_1/scene.usda"))
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn reader_resolution_rejects_symlinks_that_leave_a_twin_root() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let outside = tempfile::tempdir().expect("temporary outside root");
        let secret = outside.path().join("secret.usda");
        std::fs::write(&secret, "#usda 1.0").expect("secret");
        std::os::unix::fs::symlink(&secret, root.path().join("linked.usda")).expect("symlink");

        let roots = TwinRoots::default();
        let name = roots.register("example", root.path());
        let reader = TwinReader { roots };
        assert!(reader
            .resolve(Path::new(&format!("{name}/linked.usda")))
            .is_none());
    }
}
