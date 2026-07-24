//! Project-wide asset discovery — *which files exist*.
//!
//! One DRY scanner for "what files of extension `ext` does the project have": the
//! engine asset *library* (`<cwd>/assets`, the default/`lunco://` source) plus
//! every open Twin root (`twin://<name>/…`). Consumers (the spawn catalog for
//! `usda`, the shader catalog for `wgsl`, pickers, the API) call [`list_assets`]
//! instead of each re-walking the disk with their own scan.
//!
//! Lives in `lunco-assets` because this crate already owns *where assets live* —
//! the [`TwinRoots`](crate::twin_source::TwinRoots) registry and the `twin://` /
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
//! It used to be baked into the wasm by a `build.rs` (`BAKED_ASSET_RELS`). The
//! difference is not cosmetic: a baked listing describes the bundle the *binary was
//! compiled against*, while the manifest describes the bundle that actually
//! *shipped*. They agree right up until they don't — drop an asset into a deployed
//! `dist/` and a baked listing will never see it, with no error, because the binary
//! is certain it already knows what exists.
//!
//! So the bundle now carries its own table of contents, and nothing about the
//! assets lives in the binary.

use std::path::Path;

use bevy::prelude::*;

use crate::twin_source::TwinRoots;

/// A file discovered somewhere in the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetFile {
    /// Loadable Bevy asset path. Engine-relative (`vessels/rovers/skid_rover.usda`,
    /// served by the default source) or Twin-scoped (`twin://moonbase/structures/habitat_fsh.usda`).
    pub asset_path: String,
    /// File stem (`skid_rover`, `regolith`) — a stable id for catalogs.
    pub stem: String,
    /// Path relative to its own root (`vessels/rovers/skid_rover.usda`,
    /// `shaders/regolith.wgsl`). Use for category heuristics, and as the URL
    /// suffix the web reads the file's bytes from.
    pub rel: String,
    /// Absolute on-disk path — for native consumers that read the file's contents
    /// without re-resolving the asset path. On the web there is no filesystem, so
    /// this is the bare relative path and [`crate::asset_read`] fetches instead.
    pub abs_path: std::path::PathBuf,
    /// Open-Twin name this came from, or `None` for the engine library.
    pub twin: Option<String>,
}

/// The engine asset library's file listing — every `*.usda`/`*.wgsl` that ships.
///
/// Populated once at startup: by walking `<cwd>/assets` on native, by fetching
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
#[derive(Resource, Default)]
pub struct AssetManifest {
    rels: Vec<String>,
    ready: bool,
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

    /// Seed the listing directly. For tests and for the native walk.
    pub fn set(&mut self, rels: Vec<String>) {
        self.rels = rels;
        self.ready = true;
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
/// (`AssetVisibilitySettings`, off by default): test scenes bury the handful of
/// scenes a person actually opens, but they must stay one checkbox away —
/// hiding them from their author is how a broken one goes unnoticed.
///
/// Loading is never filtered. A scene referencing `scenarios/tests/x.rhai`
/// resolves it whether or not any browser lists it.
pub fn is_test_asset(rel: &str) -> bool {
    std::path::Path::new(rel)
        .parent()
        .is_some_and(|p| p.components().any(|c| c.as_os_str() == "tests"))
}

/// Loads [`AssetManifest`] at startup — the one place the "which files ship"
/// question is answered, per platform.
pub struct AssetDiscoveryPlugin;

impl Plugin for AssetDiscoveryPlugin {
    fn build(&self, app: &mut App) {
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

/// Native: walk `<cwd>/assets` once. The filesystem IS the manifest here — there
/// is no artifact to go stale against.
#[cfg(not(target_arch = "wasm32"))]
fn load_manifest_native(mut manifest: ResMut<AssetManifest>) {
    let dir = crate::assets_dir_abs();
    let rels = scan_library(&dir);
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
/// It used to be two: this, and an `os.walk` inlined in `scripts/build_web.sh` that
/// re-decided the extensions and the skip rules for itself. On the source `assets/`
/// tree the two agreed (86 files, exactly) — which is the trap, because they were
/// not the same rule. The shell walk descended into hidden directories; this one
/// skips them. And the manifest is generated from the **staged** `dist/assets/`
/// tree, into which `build_web.sh` copies Twins — and a Twin carries
/// `.lunco/runtime/*.usda`. So the web listing would have contained a Twin's
/// private runtime layers that native discovery never sees: a native/web
/// divergence, which is exactly what the deleted `build.rs` bake used to cause.
///
/// A packaging step must not re-derive what the runtime already defines.
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_library(dir: &Path) -> Vec<String> {
    let mut rels = Vec::new();
    walk_any(dir, dir, &mut |rel| rels.push(rel));
    rels.sort();
    rels
}

/// Web: fetch the manifest the bundle ships alongside the assets it describes.
#[cfg(target_arch = "wasm32")]
mod wasm_manifest {
    use super::*;

    /// The listing the bundle ships. Same origin, next to the wasm, generated by
    /// `build_web.sh` from the very tree it is staged into.
    const MANIFEST_URL: &str = "assets/manifest.json";

    #[derive(Resource)]
    pub struct ManifestFetch {
        tx: crossbeam_channel::Sender<Result<Vec<String>, String>>,
        rx: crossbeam_channel::Receiver<Result<Vec<String>, String>>,
    }

    impl Default for ManifestFetch {
        fn default() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self { tx, rx }
        }
    }

    pub fn start_fetch(fetch: Res<ManifestFetch>) {
        let tx = fetch.tx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // Not cached: the manifest is the ONE mutable artifact here — it
            // describes the current bundle, and a stale copy would hide a
            // freshly-deployed asset. The files it names are immutable per
            // deployment and are cached individually (see `asset_read`).
            let result = crate::web_fetch::network_fetch_uncached(MANIFEST_URL)
                .await
                .and_then(|bytes| {
                    serde_json::from_slice::<Vec<String>>(&bytes)
                        .map_err(|e| format!("{MANIFEST_URL}: not a JSON string array: {e}"))
                });
            let _ = tx.send(result);
        });
    }

    pub fn drain_fetch(fetch: Res<ManifestFetch>, mut manifest: ResMut<AssetManifest>) {
        let Ok(result) = fetch.rx.try_recv() else {
            return;
        };
        match result {
            Ok(rels) => {
                info!("ASSET_MANIFEST: {} file(s) from {MANIFEST_URL}", rels.len());
                manifest.set(rels);
            }
            Err(e) => {
                // Loud. With no manifest the browser cannot enumerate anything —
                // the spawn palette and the shader catalog come up empty — and a
                // silent empty catalog reads as "this project has no assets".
                error!(
                    "ASSET_MANIFEST: could not load {MANIFEST_URL} ({e}). \
                     The spawn/shader catalogs will be EMPTY. Is the bundle built \
                     with scripts/build_web.sh?"
                );
            }
        }
    }
}

/// List every `*.<ext>` in the project: the engine `assets/` library first, then
/// each open Twin root (sorted by name). `ext` is the bare extension without the
/// dot (`"usda"`, `"wgsl"`).
///
/// The engine library comes from [`AssetManifest`] on both platforms — so both
/// read one listing, rather than native walking and web consulting a table that
/// only native's walk could have produced. Twin roots are walked live (native
/// only; a Twin's files are not in the shipped bundle).
pub fn list_assets(manifest: &AssetManifest, roots: &TwinRoots, ext: &str) -> Vec<AssetFile> {
    let mut out = Vec::new();
    let suffix = format!(".{ext}");

    // Engine library, addressed by the default source (plain relative paths).
    #[cfg(not(target_arch = "wasm32"))]
    let assets_dir = crate::assets_dir_abs();
    for rel in manifest.rels().iter().filter(|r| r.ends_with(&suffix)) {
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
    for name in roots.names() {
        if let Some(root) = roots.root_of(&name) {
            walk(&root, &root, ext, &mut |rel| {
                out.push(AssetFile {
                    asset_path: crate::twin_uri(&name, &rel),
                    stem: stem_of(&rel),
                    abs_path: root.join(&rel),
                    twin: Some(name.clone()),
                    rel,
                });
            });
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = roots;

    out
}

/// Convenience: every `*.usda` in the project. Thin wrapper over [`list_assets`].
pub fn list_usd_assets(manifest: &AssetManifest, roots: &TwinRoots) -> Vec<AssetFile> {
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
pub fn list_scene_assets(manifest: &AssetManifest, roots: &TwinRoots) -> Vec<AssetFile> {
    let mut out = list_assets(manifest, roots, "usda");
    // One manifest read per Twin, not per asset.
    let globs: std::collections::HashMap<String, Vec<String>> = roots
        .names()
        .into_iter()
        .map(|name| {
            let g = roots
                .root_of(&name)
                .map(|root| scene_globs_of_twin(&root))
                .unwrap_or_else(default_scene_globs);
            (name, g)
        })
        .collect();

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
        None => asset.rel.starts_with("scenes/"),
    });
    out
}

fn default_scene_globs() -> Vec<String> {
    lunco_twin::DEFAULT_SCENE_GLOBS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The scene globs a Twin declares, or [`DEFAULT_SCENE_GLOBS`] if it declares
/// none (or has no readable `twin.toml` — a folder opened as a Twin root need
/// not have been promoted to one).
#[cfg(not(target_arch = "wasm32"))]
fn scene_globs_of_twin(root: &Path) -> Vec<String> {
    lunco_twin::TwinManifest::read(&root.join(lunco_twin::MANIFEST_FILENAME))
        .ok()
        .and_then(|m| m.usd.and_then(|u| u.scenes))
        .unwrap_or_else(default_scene_globs)
}

/// Web has no Twin folders to read a manifest from — Twin roots are a native
/// concept (see [`list_assets`]), so this is unreachable there.
#[cfg(target_arch = "wasm32")]
fn scene_globs_of_twin(_root: &Path) -> Vec<String> {
    default_scene_globs()
}

/// Every catalogued file under `dir`, regardless of extension — the native walk
/// that produces the manifest. Mirrors what `build_web.sh` writes for the web.
///
/// `rhai` is catalogued because scripts are importable assets: a scenario can
/// `import` a module from any asset source, and on the web the manifest is the
/// only way a file is discoverable at all. Omitting it made script modules a
/// native-only feature by accident.
#[cfg(not(target_arch = "wasm32"))]
const MANIFEST_EXTS: &[&str] = &["usda", "wgsl", "rhai"];

#[cfg(not(target_arch = "wasm32"))]
fn walk_any(base: &Path, dir: &Path, f: &mut impl FnMut(String)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if n.starts_with('.') || n == "target" => continue,
                _ => walk_any(base, &p, f),
            }
        } else if p
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|e| MANIFEST_EXTS.contains(&e))
        {
            if let Ok(rel) = p.strip_prefix(base) {
                if let Some(rel_s) = rel.to_str() {
                    f(crate::asset_path::slashed(rel_s));
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn walk(base: &Path, dir: &Path, ext: &str, f: &mut impl FnMut(String)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            match p.file_name().and_then(|s| s.to_str()) {
                Some(n) if n.starts_with('.') || n == "target" => continue,
                _ => walk(base, &p, ext, f),
            }
        } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
            if let Ok(rel) = p.strip_prefix(base) {
                if let Some(rel_s) = rel.to_str() {
                    f(crate::asset_path::slashed(rel_s));
                }
            }
        }
    }
}

fn stem_of(rel: &str) -> String {
    Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test asset is one under a `tests/` DIRECTORY, in any library. The name
    /// decides nothing: a scene called `something_test.usda` sitting beside the
    /// scenes a person opens is not one, and a scene called `sensor.usda` under
    /// `scenes/tests/` is.
    #[test]
    fn test_assets_are_identified_by_their_directory_not_their_name() {
        assert!(is_test_asset("scenes/tests/landing_legs.usda"));
        assert!(is_test_asset("scenarios/tests/landing_legs.rhai"));
        assert!(!is_test_asset("scenes/sandbox/lander_cinematic.usda"));
        assert!(!is_test_asset("behaviors/solar_rover_patrol.btxml"));
        // The suffix convention it replaces — a file that merely READS as a test
        // is still shown, because nothing but its folder makes it one.
        assert!(!is_test_asset("scenes/sandbox/something_test.usda"));
        // A file literally named `tests.usda` is a file, not a directory.
        assert!(!is_test_asset("scenes/tests.usda"));
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
            "shaders/regolith.wgsl".into(),
        ]);
        let roots = TwinRoots::default();
        let usd = list_usd_assets(&m, &roots);
        assert_eq!(usd.len(), 1);
        assert_eq!(usd[0].stem, "skid_rover");
        assert_eq!(usd[0].rel, "vessels/rovers/skid_rover.usda");
        assert_eq!(list_assets(&m, &roots, "wgsl").len(), 1);
    }
}
