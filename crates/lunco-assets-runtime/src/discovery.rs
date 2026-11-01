//! Project-wide asset discovery — *which files exist*.
//!
//! One DRY scanner for "what files of extension `ext` does the project have": the
//! engine asset *library* (the runtime `assets/` root, the default/`lunco://` source) plus
//! every open Twin root (`twin://<name>/…`). Consumers (the spawn catalog for
//! `usda`, the shader catalog for `wgsl`, pickers, the API) call [`list_assets`]
//! instead of each re-walking the disk with their own scan.
//!
//! Lives in `lunco-assets-runtime` because this crate owns catalog enumeration —
//! the [`TwinRoots`](lunco_assets_core::twin_source::TwinRoots) registry and the `twin://` /
//! `lunco://` schemes. What a file *says* is a separate question, answered by
//! reading it ([`crate::asset_read`]).
//!
//! # The listing is data, not code
//!
//! Native walks the filesystem. The browser cannot: HTTP has no `readdir`, so the
//! engine library's file list has to be *told* to it. That list is
//! **`assets/manifest.json`**, generated from the staged tree by
//! `scripts/build_web.sh` and fetched at boot.
//!
//! The manifest describes the staged bundle rather than the compiler's source
//! tree. Native runtime and packaging use the same contained inventory.

use std::path::Path;

use bevy::prelude::*;

use lunco_assets_core::twin_source::TwinRoots;

/// A file discovered somewhere in the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetFile {
    /// Loadable Bevy asset path. Engine-relative (`vessels/rovers/skid_rover.usda`,
    /// served by the default source) or Twin-scoped (`twin://moonbase/structures/habitat_fsh.usda`).
    pub asset_path: String,
    /// File stem (`skid_rover`, `regolith`) — the default display/id stem for
    /// catalogs. Catalog owners disambiguate collisions using `asset_path`.
    pub stem: String,
    /// Path relative to its own root (`vessels/rovers/skid_rover.usda`,
    /// `shaders/regolith.wesl`). Use for category heuristics, and as the URL
    /// suffix the web reads the file's bytes from.
    pub rel: String,
    /// Absolute on-disk path — for native consumers that read the file's contents
    /// without re-resolving the asset path. On the web there is no filesystem, so
    /// this is the bare relative path and [`crate::asset_read`] fetches instead.
    pub abs_path: std::path::PathBuf,
    /// Open-Twin name this came from, or `None` for the engine library.
    pub twin: Option<String>,
}

/// The engine asset library's file listing — every loadable authored source that ships.
///
/// Populated once at startup: by walking the runtime `assets/` root on native, by fetching
/// `assets/manifest.json` on the web.
///
/// Because it lands *late* on the web, consumers must not treat "not loaded" as
/// "empty". Two of them make that distinction, differently, and both are right:
///
/// - `maintain_catalogs` re-enumerates whenever this resource **changes**, so a
///   manifest that arrives on frame 40 is simply a change on frame 40. It needs no
///   readiness check at all — it has no "already scanned" state to corrupt.
/// - The UI cannot wait, since it must draw *something* — so it asks
///   [`ready`](Self::ready) and says "loading…" rather than "no scenes found".
#[derive(Resource, Default, Clone)]
pub struct AssetManifest {
    rels: Vec<String>,
    ready: bool,
    revision: u64,
}

impl AssetManifest {
    /// Whether the listing has been loaded. `false` means "not known yet", NOT
    /// "empty" — a consumer must not conclude "there are no assets" from it.
    ///
    /// Only for consumers that must render a decision *now* (the UI). A consumer
    /// that can react to the manifest arriving should do that instead — see the
    /// type docs.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// Every shipped engine-library path, relative to `assets/`.
    pub fn rels(&self) -> &[String] {
        &self.rels
    }

    /// Monotonic revision of the authored library inventory.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Seed the listing directly. For tests and for the native walk.
    pub fn set(&mut self, rels: Vec<String>) {
        self.rels = rels;
        self.ready = true;
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Every file supplied by the engine library, independent of its extension.
///
/// This is the library-browser projection of the authoritative manifest. It is
/// deliberately separate from [`list_assets`]: the latter answers a typed
/// catalog question across the library and open Twins, while this answers which
/// source files the shipped library itself exposes.
pub fn list_library_assets(
    manifest: &AssetManifest,
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    #[cfg(not(target_arch = "wasm32"))]
    let assets_dir = lunco_assets_core::assets_dir_abs().map_err(|error| {
        lunco_assets_core::TwinRootsError::AssetResolution(error.kind(), error.to_string())
    })?;

    Ok(manifest
        .rels()
        .iter()
        .map(|rel| AssetFile {
            asset_path: rel.clone(),
            stem: stem_of(rel),
            #[cfg(not(target_arch = "wasm32"))]
            abs_path: assets_dir.join(rel),
            #[cfg(target_arch = "wasm32")]
            abs_path: std::path::PathBuf::from(rel),
            twin: None,
            rel: rel.clone(),
        })
        .collect())
}

/// Resolve a registered asset address to its native file record.
///
/// The library half comes solely from [`AssetManifest`]; Twin paths are accepted
/// only when their URI names an open root and a normal relative file beneath it.
/// This is a lookup, not a second catalog walker, so the source editor cannot
/// open an arbitrary filesystem path by spelling it as an asset address.
pub fn resolve_asset(
    manifest: &AssetManifest,
    roots: &TwinRoots,
    asset_path: &str,
) -> Result<Option<AssetFile>, lunco_assets_core::TwinRootsError> {
    if let Some(asset) = list_library_assets(manifest)?
        .into_iter()
        .find(|asset| asset.asset_path == asset_path)
    {
        return Ok(Some(asset));
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some((name, rel)) = lunco_assets_core::twin_source::parse_twin_uri(asset_path) else {
            return Ok(None);
        };
        let Some(rel_path) = lunco_assets_path::relative_path(rel) else {
            return Ok(None);
        };
        let Some(abs_path) = roots.resolve_file(name, &rel_path)? else {
            return Ok(None);
        };
        Ok(Some(AssetFile {
            asset_path: asset_path.to_string(),
            stem: stem_of(rel),
            rel: rel.to_string(),
            abs_path,
            twin: Some(name.to_string()),
        }))
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = roots;
        Ok(None)
    }
}

/// Is `rel` a TEST asset — a scene or scenario that exists to be run by
/// `scene_test`, not opened by a person?
///
/// The answer is the path: anything under a `tests/` directory
/// (`scenes/tests/…`, `scenarios/tests/…`). Not the filename, and not a flag
/// inside the file — a browser has to decide before it has read anything, and a
/// `_test` suffix convention is a rule every new file has to remember.
///
/// This states the FACT. Whether a given browser shows them is a user setting
/// (`AssetVisibilitySettings`, off by default). The Scenarios menu keeps Twin
/// tests in a dedicated submenu; its library tests follow that preference.
///
/// Loading is never filtered. A scene referencing `scenarios/tests/x.rhai`
/// resolves it whether or not any browser lists it.
pub fn is_test_asset(rel: &str) -> bool {
    std::path::Path::new(rel)
        .parent()
        .is_some_and(|p| p.components().any(|c| c.as_os_str() == "tests"))
}

/// Which schema registry owns an authored engine schema source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaAssetOwner {
    /// A standard USD schema extension used by the engine.
    Core,
    /// A LunCo project schema extension.
    Lunco,
}

/// Classify one manifest-relative USDA schema source.
///
/// Schema layout is an asset-library concern. Keeping this predicate beside
/// manifest discovery prevents runtime packages from embedding the directory
/// names used by the shipped schema bundle.
pub fn schema_asset_owner(rel: &str) -> Option<SchemaAssetOwner> {
    let rel = lunco_assets_path::slashed(rel);
    let mut segments = rel.split('/');
    if segments.next() != Some("schemas") {
        return None;
    }
    let owner = match segments.next()? {
        "core" => SchemaAssetOwner::Core,
        "lunco" => SchemaAssetOwner::Lunco,
        _ => return None,
    };
    rel.ends_with(".usda").then_some(owner)
}

/// Loads [`AssetManifest`] at startup — the one place the "which files ship"
/// question is answered, per platform.
pub struct AssetDiscoveryPlugin;

impl Plugin for AssetDiscoveryPlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<AssetManifest>();
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, load_manifest_native);
        #[cfg(target_arch = "wasm32")]
        {
            app.init_resource::<wasm_manifest::ManifestFetch>();
            app.add_systems(Startup, wasm_manifest::start_fetch);
            app.add_systems(Update, wasm_manifest::drain_fetch);
        }
    }
}

/// Native: walk the runtime `assets/` root once. The filesystem IS the manifest here — there
/// is no artifact to go stale against.
#[cfg(not(target_arch = "wasm32"))]
fn load_manifest_native(mut manifest: ResMut<AssetManifest>, mut commands: Commands) {
    let dir = match lunco_assets_core::assets_dir_abs() {
        Ok(root) => root,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "asset-root-unavailable",
                error.to_string(),
            );
            return;
        }
    };
    let rels = match scan_library(&dir) {
        Ok(rels) => rels,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "asset-inventory-invalid",
                error.to_string(),
            );
            return;
        }
    };
    info!(
        "ASSET_MANIFEST: {} file(s) under {}",
        rels.len(),
        dir.display()
    );
    manifest.set(rels);
}

/// Every catalogued file under `dir`, as sorted `/`-separated relative paths.
///
/// **The one definition of "which files ship."** Native calls it to build its
/// manifest at startup; the `build_asset_manifest` binary calls it to write the
/// `manifest.json` the web build ships. One function, so the two platforms cannot
/// disagree about what an asset is.
///
/// Hidden directories and build outputs are excluded. Inventory failures reject
/// the listing; callers must not publish a partial catalog.
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_library(dir: &Path) -> std::io::Result<Vec<String>> {
    catalog_files(dir, MANIFEST_EXTS)
}

/// Inventory every file below a caller-selected native source root.
///
/// Asset consumers use this instead of implementing another recursive
/// filesystem walk. The returned paths are stable and absolute on native so
/// diagnostics retain the same URI that the source-root owner supplied.
/// Traversal and identity failures are reported separately; callers admit the
/// inventory atomically while surfacing every failed path.
#[cfg(not(target_arch = "wasm32"))]
pub fn inventory_files(root: &Path) -> (Vec<std::path::PathBuf>, Vec<String>) {
    inventory_files_with(root, |_, _| true)
}

#[cfg(not(target_arch = "wasm32"))]
fn inventory_files_with(
    root: &Path,
    include: impl Fn(&Path, Option<lunco_storage::StorageEntryKind>) -> bool,
) -> (Vec<std::path::PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    if !root.is_absolute() {
        return (
            files,
            vec![format!(
                "source root must be an admitted absolute directory: {}",
                root.display()
            )],
        );
    }
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let identity = match lunco_storage::canonicalize_file_path(&dir) {
            Ok(identity) => identity,
            Err(error) => {
                diagnostics.push(format!("cannot identify {}: {error}", dir.display()));
                continue;
            }
        };
        if !visited.insert(identity) {
            diagnostics.push(format!(
                "source directory is reachable more than once: {}",
                dir.display()
            ));
            continue;
        }
        let entries = match lunco_storage::read_directory_sync(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                diagnostics.push(format!("cannot read {}: {error}", dir.display()));
                continue;
            }
        };
        for path in entries {
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative,
                Err(error) => {
                    diagnostics.push(format!(
                        "source entry {} has no owning root: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let kind = match lunco_storage::entry_kind_file_sync(&path) {
                Ok(kind) => kind,
                Err(lunco_storage::StorageError::Unsupported(_)) if !include(relative, None) => {
                    continue;
                }
                Err(error) => {
                    diagnostics.push(format!("cannot inspect {}: {error}", path.display()));
                    continue;
                }
            };
            if !include(relative, Some(kind)) {
                continue;
            }
            let resolved = match lunco_assets_core::existing_path_within_root(root, relative) {
                Ok(Some(resolved)) => resolved,
                Ok(None) => {
                    diagnostics.push(format!("source entry disappeared: {}", path.display()));
                    continue;
                }
                Err(error) => {
                    diagnostics.push(format!("cannot resolve {}: {error}", path.display()));
                    continue;
                }
            };
            match kind {
                lunco_storage::StorageEntryKind::Directory => stack.push(path),
                lunco_storage::StorageEntryKind::File => {
                    if !visited.insert(resolved) {
                        diagnostics.push(format!(
                            "source file is reachable more than once: {}",
                            path.display()
                        ));
                        continue;
                    }
                    if path.to_str().is_none() {
                        diagnostics.push(format!(
                            "source path cannot be represented as a UTF-8 identity: {}",
                            path.display()
                        ));
                        continue;
                    }
                    files.push(path);
                }
                _ => {}
            }
        }
    }
    files.sort();
    diagnostics.sort();
    (files, diagnostics)
}

/// Read selected UTF-8 sources from the common containment-checked inventory.
/// Returned source addresses retain native absolute path syntax, including
/// Windows verbatim prefixes. Consumers derive portable identities only after
/// stripping the native root; slash conversion of an absolute verbatim path
/// changes how Windows parses its prefix.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_files_with_extension(
    root: &Path,
    extension: &str,
) -> (Vec<(String, String)>, Vec<String>) {
    let (paths, mut diagnostics) = inventory_files(root);
    let mut files = Vec::new();
    for path in paths.into_iter().filter(|path| {
        path.extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    }) {
        // Inventory is an identity snapshot. Byte reads revalidate containment
        // against the current filesystem and consume the resolved path.
        let source = path
            .strip_prefix(root)
            .map_err(std::io::Error::other)
            .and_then(|relative| {
                lunco_assets_core::existing_path_within_root(root, relative)?
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            })
            .and_then(|resolved| lunco_assets_core::read_asset_file_string(&resolved));
        match source {
            Ok(source) => files.push((
                path.to_str()
                    .expect("inventory validated UTF-8 paths")
                    .to_owned(),
                source,
            )),
            Err(error) => diagnostics.push(format!("cannot read {}: {error}", path.display())),
        }
    }
    diagnostics.sort();
    (files, diagnostics)
}

/// A filesystem source root is unavailable on wasm. Callers receive a
/// terminal diagnostic rather than entering a native filesystem path.
#[cfg(target_arch = "wasm32")]
pub fn read_files_with_extension(
    root: &Path,
    _extension: &str,
) -> (Vec<(String, String)>, Vec<String>) {
    (
        Vec::new(),
        vec![format!(
            "cannot read source root {} on wasm",
            root.display()
        )],
    )
}

/// Web: fetch the manifest the bundle ships alongside the assets it describes.
#[cfg(target_arch = "wasm32")]
mod wasm_manifest {
    use super::*;

    /// The listing the bundle ships. Same origin, next to the wasm, generated by
    /// `build_web.sh` from the very tree it is staged into.
    #[derive(Resource)]
    pub(super) struct ManifestFetch {
        tx: crossbeam_channel::Sender<Result<Vec<String>, String>>,
        rx: crossbeam_channel::Receiver<Result<Vec<String>, String>>,
    }

    impl Default for ManifestFetch {
        fn default() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self { tx, rx }
        }
    }

    pub(super) fn start_fetch(
        fetch: Res<ManifestFetch>,
        settings: Res<lunco_settings::DownloadSettings>,
    ) {
        let tx = fetch.tx.clone();
        let settings = settings.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // Not cached: the manifest is the ONE mutable artifact here — it
            // describes the current bundle, and a stale copy would hide a
            // freshly-deployed asset. The files it names are immutable per
            // deployment and are cached individually (see `asset_read`).
            let manifest_url = lunco_assets_core::asset_manifest_url();
            let result =
                lunco_assets_core::web_fetch::network_fetch_uncached(&manifest_url, &settings)
                    .await
                    .and_then(|bytes| {
                        serde_json::from_slice::<Vec<String>>(&bytes)
                            .map_err(|e| format!("{manifest_url}: not a JSON string array: {e}"))
                    });
            let _ = tx.send(result);
        });
    }

    pub(super) fn drain_fetch(fetch: Res<ManifestFetch>, mut manifest: ResMut<AssetManifest>) {
        let Ok(result) = fetch.rx.try_recv() else {
            return;
        };
        match result {
            Ok(rels) => {
                let manifest_url = lunco_assets_core::asset_manifest_url();
                info!("ASSET_MANIFEST: {} file(s) from {manifest_url}", rels.len());
                manifest.set(rels);
            }
            Err(e) => {
                // Loud. With no manifest the browser cannot enumerate anything —
                // the spawn palette and the shader catalog come up empty — and a
                // silent empty catalog reads as "this project has no assets".
                let manifest_url = lunco_assets_core::asset_manifest_url();
                error!(
                    "ASSET_MANIFEST: could not load {manifest_url} ({e}). \
                     The spawn/shader catalogs will be EMPTY. Is the bundle built \
                     with scripts/build_web.sh?"
                );
            }
        }
    }
}

/// List every `*.<ext>` in the project: the engine `assets/` library first, then
/// each open Twin root (sorted by name). `ext` is the bare extension without the
/// dot (`"usda"`, `"wesl"`).
///
/// The engine library comes from [`AssetManifest`] on both platforms — so both
/// read one listing, rather than native walking and web consulting a table that
/// only native's walk could have produced. Twin roots are walked live (native
/// only; a Twin's files are not in the shipped bundle).
pub fn list_assets(
    manifest: &AssetManifest,
    roots: &TwinRoots,
    ext: &str,
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    list_assets_with_extensions(manifest, roots, &[ext])
}

/// List every project asset whose extension is in `extensions` with one shared
/// traversal of the engine library and each open Twin. Callers that need more
/// than one catalog projection should use this instead of walking the same
/// roots once per extension.
pub fn list_assets_with_extensions(
    manifest: &AssetManifest,
    roots: &TwinRoots,
    extensions: &[&str],
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    let mut out = Vec::new();
    let suffixes: Vec<String> = extensions.iter().map(|ext| format!(".{ext}")).collect();

    // Engine library, addressed by the default source (plain relative paths).
    #[cfg(not(target_arch = "wasm32"))]
    let assets_dir = lunco_assets_core::assets_dir_abs().map_err(|error| {
        lunco_assets_core::TwinRootsError::AssetResolution(error.kind(), error.to_string())
    })?;
    for rel in manifest
        .rels()
        .iter()
        .filter(|rel| suffixes.iter().any(|suffix| rel.ends_with(suffix)))
    {
        out.push(AssetFile {
            asset_path: rel.clone(),
            stem: stem_of(rel),
            #[cfg(not(target_arch = "wasm32"))]
            abs_path: assets_dir.join(rel),
            #[cfg(target_arch = "wasm32")]
            abs_path: std::path::PathBuf::from(rel),
            twin: None,
            rel: rel.clone(),
        });
    }

    // Open Twins → `twin://<name>/<rel>` so the `twin://` reader resolves them.
    // Native only: a Twin lives on disk, and the web has no filesystem to walk.
    #[cfg(not(target_arch = "wasm32"))]
    for name in roots.names()? {
        if let Some(root) = roots.root_of(&name)? {
            for rel in catalog_files(&root, extensions).map_err(|error| {
                lunco_assets_core::TwinRootsError::AssetResolution(error.kind(), error.to_string())
            })? {
                out.push(AssetFile {
                    asset_path: lunco_assets_core::twin_uri(&name, &rel),
                    stem: stem_of(&rel),
                    abs_path: root.join(&rel),
                    twin: Some(name.clone()),
                    rel,
                });
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = roots;

    Ok(out)
}

/// Convenience: every `*.usda` in the project. Thin wrapper over [`list_assets`].
pub fn list_usd_assets(
    manifest: &AssetManifest,
    roots: &TwinRoots,
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    list_assets(manifest, roots, "usda")
}

/// Every `*.usda` in the project that is a **loadable scene** — what a Scenarios
/// menu offers, as opposed to the vessels, structures, looks and library layers
/// that exist to be referenced by one.
///
/// **A project says where its scenes are; the engine does not guess.** Each open
/// Twin answers for itself via `[usd] scenes` in its `twin.toml`
/// ([`UsdManifest::scenes`](lunco_twin::UsdManifest::scenes)); the
/// engine library uses its own `scenes/` layout. Undeclared Twins fall back to
/// [`DEFAULT_SCENE_GLOBS`].
///
/// This replaced a `rel.starts_with("scenes/")` test applied to every asset from
/// every source — the engine library's folder layout imposed on projects that do
/// not share it. A Twin keeping scenes in `sim/scenes/` had none of them listed:
/// its own scene could be on screen while the menu said the project had none.
pub fn list_scene_assets(
    manifest: &AssetManifest,
    roots: &TwinRoots,
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    let mut out = list_assets(manifest, roots, "usda")?;
    // One manifest read per Twin, not per asset.
    let globs: std::collections::HashMap<String, Vec<String>> = roots
        .names()?
        .into_iter()
        .map(|name| {
            let g = match roots.root_of(&name)? {
                Some(root) => scene_globs_of_twin(&root)?,
                None => default_scene_globs(),
            };
            Ok((name, g))
        })
        .collect::<Result<_, lunco_assets_core::TwinRootsError>>()?;

    out.retain(|asset| match &asset.twin {
        Some(name) => globs
            .get(name)
            .map(|g| {
                g.iter()
                    .any(|glob| lunco_twin::glob_matches(glob, &asset.rel))
            })
            .unwrap_or(false),
        // The engine library's own layout, which it is entitled to assert about
        // itself — it ships the folder.
        None => lunco_assets_core::is_engine_scene_asset(&asset.rel),
    });
    Ok(out)
}

/// Scene choices from already-admitted workspace inventories, without walking
/// roots or rereading manifests on the presentation thread.
pub fn list_indexed_scene_assets(
    manifest: &AssetManifest,
    roots: &TwinRoots,
    twins: &[&lunco_twin::Twin],
) -> Result<Vec<AssetFile>, lunco_assets_core::TwinRootsError> {
    let mut assets = list_library_assets(manifest)?;
    assets.retain(|asset| {
        asset.rel.ends_with(".usda") && lunco_assets_core::is_engine_scene_asset(&asset.rel)
    });
    for twin in twins {
        let authority = roots.name_for_root(&twin.root)?.ok_or_else(|| {
            lunco_assets_core::TwinRootsError::AssetResolution(
                std::io::ErrorKind::NotFound,
                format!("Workspace Twin is not mounted: {}", twin.root.display()),
            )
        })?;
        let globs = scene_globs_from_manifest(twin.manifest.as_ref());
        for file in twin.files() {
            let rel = logical_catalog_path(&file.relative_path).map_err(|error| {
                lunco_assets_core::TwinRootsError::AssetResolution(error.kind(), error.to_string())
            })?;
            let matches = globs
                .iter()
                .any(|glob| lunco_twin::glob_matches(glob, &rel));
            if rel.ends_with(".usda") && matches {
                assets.push(AssetFile {
                    asset_path: lunco_assets_core::twin_uri(&authority, &file.relative_path),
                    stem: stem_of(&rel),
                    abs_path: twin.root.join(&file.relative_path),
                    rel,
                    twin: Some(authority.clone()),
                });
            }
        }
    }
    Ok(assets)
}

fn default_scene_globs() -> Vec<String> {
    lunco_twin::DEFAULT_SCENE_GLOBS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn scene_globs_from_manifest(manifest: Option<&lunco_twin::TwinManifest>) -> Vec<String> {
    manifest
        .and_then(|manifest| manifest.usd.as_ref())
        .and_then(|usd| usd.scenes.clone())
        .unwrap_or_else(default_scene_globs)
}

/// The scene globs a Twin declares, or [`DEFAULT_SCENE_GLOBS`] when the Twin
/// has no `twin.toml`. An unreadable or malformed manifest is an invalid
/// discovery boundary and is returned to the caller instead of being treated
/// as an undeclared Twin.
#[cfg(not(target_arch = "wasm32"))]
fn scene_globs_of_twin(root: &Path) -> Result<Vec<String>, lunco_assets_core::TwinRootsError> {
    let manifest = root.join(lunco_twin::MANIFEST_FILENAME);
    match lunco_twin::TwinManifest::read(&manifest) {
        Ok(manifest) => Ok(scene_globs_from_manifest(Some(&manifest))),
        Err(lunco_twin::TwinError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(default_scene_globs())
        }
        Err(error) => Err(lunco_assets_core::TwinRootsError::AssetResolution(
            std::io::ErrorKind::InvalidData,
            format!("cannot read {}: {error}", manifest.display()),
        )),
    }
}

/// Web has no Twin folders to read a manifest from — Twin roots are a native
/// concept (see [`list_assets`]), so this is unreachable there.
#[cfg(target_arch = "wasm32")]
fn scene_globs_of_twin(_root: &Path) -> Result<Vec<String>, lunco_assets_core::TwinRootsError> {
    Ok(default_scene_globs())
}

/// Every catalogued file under `dir`, regardless of extension — the native walk
/// that produces the manifest. Mirrors what `build_web.sh` writes for the web.
///
/// `rhai` is catalogued because scripts are importable assets: a scenario can
/// `import` a module from any asset source, and on the web the manifest is the
/// only way a file is discoverable at all. Omitting it made script modules a
/// native-only feature by accident.
#[cfg(not(target_arch = "wasm32"))]
const MANIFEST_EXTS: &[&str] = &["json", "rhai", "usda", "wesl"];

#[cfg(not(target_arch = "wasm32"))]
fn catalog_files(dir: &Path, extensions: &[&str]) -> std::io::Result<Vec<String>> {
    let root = lunco_storage::canonicalize_file_path(dir).map_err(|error| match error {
        lunco_storage::StorageError::Io(error) => error,
        error => std::io::Error::other(error.to_string()),
    })?;
    let (files, diagnostics) = inventory_files_with(&root, |relative, kind| {
        if kind == Some(lunco_storage::StorageEntryKind::Directory) {
            return relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !name.starts_with('.') && name != "target");
        }
        relative
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
    });
    if !diagnostics.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            diagnostics.join("\n"),
        ));
    }
    files
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&root).map_err(std::io::Error::other)?;
            logical_catalog_path(relative)
        })
        .collect()
}

fn logical_catalog_path(relative: &Path) -> std::io::Result<String> {
    // A literal backslash in a Unix filename must not become another directory.
    let rel = relative
        .to_str()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "catalog path is not UTF-8")
        })?
        .replace(std::path::MAIN_SEPARATOR, "/");
    if !lunco_assets_path::is_safe_relative_path(&rel) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "catalog path cannot be represented as a logical asset: {}",
                relative.display()
            ),
        ));
    }
    Ok(rel)
}

fn stem_of(rel: &str) -> String {
    Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Construct an owned directory-link fixture without interpolating native paths into shell code.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn link_test_directory(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "New-Item -ItemType Junction -Path $env:LUNCO_TEST_LINK -Target $env:LUNCO_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("LUNCO_TEST_LINK", link)
            .env("LUNCO_TEST_TARGET", target)
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn catalog_inventory_contract() {
        const CHILD: &str = "LUNCO_CATALOG_INVENTORY_TEST";
        if let Ok(root) = std::env::var(CHILD) {
            let roots = TwinRoots::default();
            roots
                .register("probe", std::path::PathBuf::from(&root))
                .unwrap();
            let result = list_assets(&AssetManifest::default(), &roots, "rhai");
            let error = result.expect_err("an escaped catalog entry must reject discovery");
            assert!(error.to_string().contains("outside"), "{error}");
            assert!(error.to_string().contains("more than once"), "{error}");
            assert!(scan_library(Path::new(&root)).is_err());
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("catalog # % 日本語");
        let outside = folder.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("visible.rhai"), "// visible").unwrap();
        std::fs::write(root.join(".hidden.rhai"), "// visible file").unwrap();
        for name in [".private", "target"] {
            std::fs::create_dir(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("hidden.rhai"), "// hidden directory").unwrap();
        }
        std::fs::write(outside.join("outside.rhai"), "// outside").unwrap();
        #[cfg(unix)]
        {
            let result = std::process::Command::new("mkfifo")
                .arg(root.join("ignored.pipe"))
                .status()
                .unwrap();
            assert!(result.success());
        }
        link_test_directory(&root.join(".excluded"), &outside);
        assert_eq!(
            scan_library(&root).unwrap(),
            [".hidden.rhai", "visible.rhai"]
        );
        assert!(scan_library(&root.join("missing")).is_err());
        assert!(scan_library(&root.join("visible.rhai")).is_err());
        #[cfg(unix)]
        for (index, name) in [r"literal\name.rhai", "literal:name.rhai"]
            .into_iter()
            .enumerate()
        {
            let invalid = folder.path().join(format!("invalid-{index}"));
            std::fs::create_dir(&invalid).unwrap();
            std::fs::write(invalid.join(name), "// native name").unwrap();
            assert!(scan_library(&invalid).is_err());
        }
        link_test_directory(&root.join("escape"), &outside);
        link_test_directory(&root.join("cycle"), &root);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "discovery::tests::catalog_inventory_contract",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env(lunco_assets_core::ASSET_ROOT_ENV, &root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    /// A test asset is one under a `tests/` DIRECTORY, in any library. The name
    /// decides nothing: a scene called `something_test.usda` sitting beside the
    /// scenes a person opens is not one, and a scene called `sensor.usda` under
    /// `scenes/tests/` is.
    #[test]
    fn test_assets_are_identified_by_their_directory_not_their_name() {
        assert!(is_test_asset("scenes/tests/landing_legs.usda"));
        assert!(is_test_asset("scenarios/tests/landing_legs.rhai"));
        assert!(!is_test_asset("scenes/luncosim/lander_cinematic.usda"));
        assert!(!is_test_asset("behaviors/legacy_tree.xml"));
        // The suffix convention it replaces — a file that merely READS as a test
        // is still shown, because nothing but its folder makes it one.
        assert!(!is_test_asset("scenes/luncosim/something_test.usda"));
        // A file literally named `tests.usda` is a file, not a directory.
        assert!(!is_test_asset("scenes/tests.usda"));
    }

    #[test]
    fn schema_sources_are_classified_by_the_asset_owner() {
        assert_eq!(
            schema_asset_owner("schemas/core/UsdPhysics.usda"),
            Some(SchemaAssetOwner::Core)
        );
        assert_eq!(
            schema_asset_owner("schemas/lunco/LunCoCamera.usda"),
            Some(SchemaAssetOwner::Lunco)
        );
        assert_eq!(schema_asset_owner("schemas/custom/Thing.usda"), None);
        assert_eq!(schema_asset_owner("schemas/core/Thing.usda.bak"), None);
    }

    /// An unready manifest is "not known yet", not "empty". Nothing may conclude
    /// there are no assets from a listing that has not arrived.
    #[test]
    fn unready_manifest_is_distinguishable_from_an_empty_one() {
        let mut m = AssetManifest::default();
        assert!(!m.ready());
        assert!(m.rels().is_empty());
        m.set(Vec::new());
        assert!(m.ready());
        assert!(m.rels().is_empty());
    }

    #[test]
    fn lists_only_the_requested_extension() {
        let mut m = AssetManifest::default();
        m.set(vec![
            "vessels/rovers/skid_rover.usda".into(),
            "shaders/regolith.wesl".into(),
        ]);
        let roots = TwinRoots::default();
        let usd = list_usd_assets(&m, &roots).expect("list assets");
        assert_eq!(usd.len(), 1);
        assert_eq!(usd[0].stem, "skid_rover");
        assert_eq!(usd[0].rel, "vessels/rovers/skid_rover.usda");
        assert_eq!(
            list_assets(&m, &roots, "wesl").expect("list assets").len(),
            1
        );
    }

    #[test]
    fn lists_multiple_extensions_in_one_projection() {
        let mut m = AssetManifest::default();
        m.set(vec![
            "vessels/rovers/skid_rover.usda".into(),
            "shaders/regolith.wesl".into(),
            "scenarios/landing.rhai".into(),
        ]);
        let assets = list_assets_with_extensions(&m, &TwinRoots::default(), &["usda", "wesl"])
            .expect("list assets");
        assert_eq!(
            assets
                .iter()
                .map(|asset| asset.rel.as_str())
                .collect::<Vec<_>>(),
            vec!["vessels/rovers/skid_rover.usda", "shaders/regolith.wesl"]
        );
    }
}

#[cfg(all(test, unix))]
mod source_root_contract_tests {
    use super::*;
    #[test]
    fn asset_resolution_contract() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("literal # %.mo"), "model A end A;").unwrap();
        let (files, errors) = read_files_with_extension(&root, "mo");
        assert_eq!(files.len(), 1);
        assert!(errors.is_empty());
        std::fs::write(
            folder.path().join("outside.mo"),
            "model Outside end Outside;",
        )
        .unwrap();
        std::os::unix::fs::symlink(folder.path().join("outside.mo"), root.join("escape.mo"))
            .unwrap();
        std::os::unix::fs::symlink(&root, root.join("cycle")).unwrap();
        let (files, errors) = read_files_with_extension(&root, "mo");
        assert_eq!(files.len(), 1);
        assert!(
            errors.iter().any(|error| error.contains("outside")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("more than once")),
            "{errors:?}"
        );
        assert!(read_files_with_extension(Path::new("relative"), "mo").1[0].contains("absolute"));
    }

    // Linux permits byte filenames that the macOS runner's filesystem rejects
    // at creation. Exercise discovery's UTF-8 boundary where that input exists.
    #[cfg(target_os = "linux")]
    #[test]
    fn source_inventory_rejects_non_utf8_native_filenames() {
        let folder = tempfile::tempdir().unwrap();
        use std::os::unix::ffi::OsStringExt;
        let invalid_name = std::ffi::OsString::from_vec(b"invalid\xff.mo".to_vec());
        std::fs::write(
            folder.path().join(invalid_name),
            "model Invalid end Invalid;",
        )
        .unwrap();
        let (files, errors) = read_files_with_extension(folder.path(), "mo");
        assert!(files.is_empty());
        assert!(
            errors.iter().any(|error| error.contains("UTF-8 identity")),
            "{errors:?}"
        );
    }
}
