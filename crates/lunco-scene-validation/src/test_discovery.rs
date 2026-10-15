//! Discovery and classification for authored scene tests.
//!
//! A test's execution domain belongs to its test program, not to the USD scene
//! that happens to host it.  USD still owns the binding (`LunCoProgramAPI` +
//! `info:sourceAsset`); the Rhai source owns whether the assertion is headless
//! or needs the graphics stack.
//!
//! The declaration is deliberately a top-level literal constant:
//!
//! ```rhai
//! const TEST_KIND = "graphics";
//! // or `"render-contract"` for GPU diagnostics without a pixel take,
//! // or `"editor"` for workflows that require the production workbench.
//! ```
//!
//! This lets a runner classify a test without executing user code.  An omitted
//! declaration means `headless`, which keeps existing scenario tests concise
//! while making graphics tests an explicit opt-in.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lunco_usd_bevy_core::program;
use lunco_usd_bevy_stage::UsdRead;

/// The Rhai constant read by the test discovery pass.
pub const TEST_KIND_CONST: &str = "TEST_KIND";
/// The default and explicit value for CPU-only deterministic tests.
pub const HEADLESS_TEST_KIND: &str = "headless";
/// The value for tests whose assertion consumes rendered pixels or UI output.
pub const GRAPHICS_TEST_KIND: &str = "graphics";
/// The value for tests whose assertion consumes GPU/render diagnostics but no
/// pixel capture. These tests still run through the production offscreen GPU
/// host, but do not require a valid color-phase item.
pub const RENDER_CONTRACT_TEST_KIND: &str = "render-contract";
/// The value for tests that require the windowed production editor.
pub const EDITOR_TEST_KIND: &str = "editor";

/// Which runtime a scene test needs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SceneTestKind {
    /// No GPU or window/render world is required.
    Headless,
    /// Run through the offscreen renderer and inspect graphics output.
    Graphics,
    /// Run through the offscreen renderer and inspect render diagnostics.
    RenderContract,
    /// Run through the windowed production editor and inspect its UI/domain APIs.
    Editor,
}

impl SceneTestKind {
    /// Returns the stable command-line spelling used by `luncosim test --list`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Headless => HEADLESS_TEST_KIND,
            Self::Graphics => GRAPHICS_TEST_KIND,
            Self::RenderContract => RENDER_CONTRACT_TEST_KIND,
            Self::Editor => EDITOR_TEST_KIND,
        }
    }
}

/// One scene and the execution domain declared by its authored test program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneTest {
    /// The authored USD scene that binds the test observer.
    pub scene_path: PathBuf,
    /// The runtime domain selected by the observer's static declaration.
    pub kind: SceneTestKind,
}

/// Classify one Rhai source without running it.
pub fn classify_rhai_source(source: &str) -> Result<SceneTestKind, String> {
    // This is a parser-only pass, but it still uses the workspace Rhai resource
    // policy. Discovery must not turn an authored source file into an
    // unbounded parser input merely because it is only being catalogued.
    let mut engine = rhai::Engine::new_raw();
    lunco_hooks_rhai::rhai_limits::apply(&mut engine);
    let ast = engine
        .compile(source)
        .map_err(|error| format!("Rhai test source does not compile: {error}"))?;

    let declaration = ast
        .iter_literal_variables(true, true)
        .find(|(name, _, _)| *name == TEST_KIND_CONST);

    let Some((_, is_const, value)) = declaration else {
        return Ok(SceneTestKind::Headless);
    };

    if !is_const {
        return Err(format!(
            "`{TEST_KIND_CONST}` must be a top-level const with a literal string value"
        ));
    }

    let value = value.into_string().map_err(|_| {
        format!(
            "`{TEST_KIND_CONST}` must be `\"headless\"`, `\"graphics\"`, `\"render-contract\"`, or `\"editor\"`"
        )
    })?;
    match value.as_str() {
        HEADLESS_TEST_KIND => Ok(SceneTestKind::Headless),
        GRAPHICS_TEST_KIND => Ok(SceneTestKind::Graphics),
        RENDER_CONTRACT_TEST_KIND => Ok(SceneTestKind::RenderContract),
        EDITOR_TEST_KIND => Ok(SceneTestKind::Editor),
        _ => Err(format!(
            "`{TEST_KIND_CONST}` has unsupported value {value:?}; expected `{HEADLESS_TEST_KIND}`, `{GRAPHICS_TEST_KIND}`, `{RENDER_CONTRACT_TEST_KIND}`, or `{EDITOR_TEST_KIND}`"
        )),
    }
}

/// Discover every test scene recursively below `scenes_dir` through composed USD reads.
///
/// Only `.rhai` programs below an asset `tests/` directory are test observers
/// (`scenarios/tests/` in the shipped library). USD assets without such an
/// observer are not test roots; this permits referenced support layers under
/// the same asset tree. A scene may also carry a production/tutorial Rhai
/// program, which is intentionally ignored here. Multiple test observers must
/// agree on their domain so the runner never has to guess which half of a scene
/// it should execute.
pub fn discover_scene_tests(scenes_dir: &Path) -> Result<Vec<SceneTest>, String> {
    let mut scenes = scene_paths(scenes_dir)?;
    scenes.sort();

    let mut discovered = Vec::with_capacity(scenes.len());
    for scene_path in scenes {
        if let Some(test) = discover_scene_test(&scene_path)? {
            discovered.push(test);
        }
    }
    Ok(discovered)
}

#[cfg(not(target_arch = "wasm32"))]
fn scene_paths(scenes_dir: &Path) -> Result<Vec<PathBuf>, String> {
    match lunco_storage::entry_kind_file_sync(scenes_dir) {
        Ok(lunco_storage::StorageEntryKind::Directory) => {}
        Ok(lunco_storage::StorageEntryKind::File)
        | Ok(lunco_storage::StorageEntryKind::Symlink) => {
            return Err(format!(
                "scene-test root is not a directory: {}",
                scenes_dir.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "cannot read scene-test directory {}: {error}",
                scenes_dir.display()
            ));
        }
    }
    Ok(lunco_assets_runtime::discovery::scan_library(scenes_dir)
        .into_iter()
        .filter(|relative| relative.ends_with(".usda"))
        .map(|relative| scenes_dir.join(relative))
        .collect())
}

#[cfg(target_arch = "wasm32")]
fn scene_paths(_scenes_dir: &Path) -> Result<Vec<PathBuf>, String> {
    Err("scene-test discovery requires the native asset catalog".to_string())
}

fn discover_scene_test(scene_path: &Path) -> Result<Option<SceneTest>, String> {
    // Use the same Twin provenance and prepared asset reads as scene loading.
    // Native file composition alone has no Twin root for USD search paths.
    let root = lunco_twin::root_for_file(scene_path);
    let roots = lunco_assets_core::TwinRoots::default();
    let twin = match lunco_twin::TwinMode::open(&root)
        .map_err(|error| format!("{}: cannot open test root: {error}", scene_path.display()))?
    {
        lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
        lunco_twin::TwinMode::Orphan(_) => {
            return Err(format!(
                "{}: test root is not a folder",
                scene_path.display()
            ));
        }
    };
    let authority = roots
        .register_twin(&twin)
        .map_err(|error| format!("{}: cannot mount test root: {error}", scene_path.display()))?;
    let relative = scene_path
        .strip_prefix(&root)
        .map_err(|error| format!("{}: invalid test root: {error}", scene_path.display()))?;
    let identifier = lunco_assets_core::twin_uri(&authority, relative);
    let assets_root = lunco_assets_core::shipped_asset_root(scene_path);
    let limits = lunco_usd_compose::recipe::StageClosureLimits::default();
    let bytes = lunco_assets_core::read_asset_bytes_bounded_with_twin_root(
        &identifier,
        assets_root,
        Some(&root),
        limits.max_bytes,
    )
    .map_err(|error| format!("{}: cannot read scene: {error}", scene_path.display()))?;
    let recipe = lunco_usd_compose::recipe_from_bytes_with_roots(
        &identifier,
        bytes,
        assets_root,
        Some(&root),
        limits,
    )
    .map_err(|error| format!("{}: cannot prepare scene: {error}", scene_path.display()))?;
    let mut stage = lunco_usd_bevy_stage::canonical::CanonicalStage::from_recipe(&recipe)
        .map_err(|error| format!("{}: cannot compose scene: {error}", scene_path.display()))?;
    let references = lunco_usd_bevy_stage::native_paths::native_references_for_prims(
        &stage.view(),
        stage.view().prim_paths(),
    );
    let origin =
        lunco_assets_core::asset_path::load_asset_path(&identifier, None, Some(&roots), None)
            .map_err(|error| {
                format!("{}: invalid scene identity: {error}", scene_path.display())
            })?;
    stage.set_native_asset_paths(std::sync::Arc::new(
        lunco_assets_core::asset_path::PreparedAssetPaths::prepare_on_worker(
            references,
            Some(origin),
            Some(&roots),
        ),
    ));
    let view = stage.view();
    let mut kinds = BTreeSet::new();
    let mut sources = BTreeSet::new();

    for prim in view.prim_paths() {
        if !view.has_api_schema(&prim, "LunCoProgramAPI") {
            continue;
        }
        let resolved = match program::resolve_program(&view, &prim) {
            Ok(resolved) => resolved,
            Err(issue) => {
                return Err(format!(
                    "{}: unresolved program {} at {}: {}",
                    scene_path.display(),
                    prim,
                    issue.property,
                    issue.message
                ));
            }
        };
        let program::ResolvedProgram {
            backend: program::ProgramBackend::Rhai,
            source: program::ProgramSource::Asset(source_asset),
        } = resolved
        else {
            continue;
        };
        if !source_asset.ends_with(".rhai") || !sources.insert(source_asset.clone()) {
            continue;
        }
        let source_path =
            match lunco_assets_core::parse_twin_uri(&source_asset) {
                Some((authority, relative)) => roots
                    .resolve_file(authority, Path::new(relative))
                    .map_err(|error| {
                    format!("{scene_path:?}: invalid test source {source_asset}: {error}")
                })?,
                None => lunco_assets_core::id_to_disk_path(&source_asset, assets_root).map_err(
                    |error| format!("{scene_path:?}: invalid test source {source_asset}: {error}"),
                )?,
            }
            .ok_or_else(|| format!("{scene_path:?}: cannot resolve test source {source_asset}"))?;
        if !lunco_assets_runtime::discovery::is_test_asset(&source_path.to_string_lossy()) {
            continue;
        }
        let source = lunco_assets_core::read_asset_file_string(&source_path).map_err(|error| {
            format!(
                "{}: cannot read test source {}: {error}",
                scene_path.display(),
                source_path.display()
            )
        })?;
        let kind = classify_rhai_source(&source).map_err(|error| {
            format!(
                "{}: invalid test declaration in {}: {error}",
                scene_path.display(),
                source_path.display()
            )
        })?;
        kinds.insert(kind);
    }

    let kind = match kinds.len() {
        0 => return Ok(None),
        1 => *kinds.first().expect("one kind exists"),
        _ => {
            return Err(format!(
                "{}: test Rhai programs disagree on TEST_KIND",
                scene_path.display()
            ));
        }
    };

    Ok(Some(SceneTest {
        scene_path: scene_path.to_path_buf(),
        kind,
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[cfg(not(target_arch = "wasm32"))]
    use super::discover_scene_test;
    use super::{SceneTestKind, classify_rhai_source, scene_paths};

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn invalid_file_uri_test_source_is_a_terminal_discovery_error() {
        let root = tempfile::tempdir().unwrap();
        let scene = root.path().join("scene.usda");
        lunco_storage::write_file_sync(
            &scene,
            br#"#usda 1.0
 def Scope "Program" (prepend apiSchemas = ["LunCoProgramAPI"])
 {
     uniform token info:implementationSource = "sourceAsset"
     uniform asset info:sourceAsset = @file:///tests/invalid%00.rhai@
 }
"#,
        )
        .unwrap();
        let error = discover_scene_test(&scene).unwrap_err();
        assert!(error.contains("invalid file URI"), "{error}");
    }

    #[test]
    fn omitted_kind_is_headless() {
        assert_eq!(
            classify_rhai_source("fn on_start(me, ctx) { let value = me; }").unwrap(),
            SceneTestKind::Headless
        );
    }

    #[test]
    fn literal_graphics_kind_is_static_and_deterministic() {
        assert_eq!(
            classify_rhai_source("const TEST_KIND = \"graphics\";").unwrap(),
            SceneTestKind::Graphics
        );
    }

    #[test]
    fn literal_editor_kind_is_static_and_deterministic() {
        assert_eq!(
            classify_rhai_source("const TEST_KIND = \"editor\";").unwrap(),
            SceneTestKind::Editor
        );
    }

    #[test]
    fn literal_render_contract_kind_is_static_and_deterministic() {
        assert_eq!(
            classify_rhai_source("const TEST_KIND = \"render-contract\";").unwrap(),
            SceneTestKind::RenderContract
        );
    }

    #[test]
    fn invalid_kind_is_rejected() {
        let error = classify_rhai_source("const TEST_KIND = \"windowed\";").unwrap_err();
        assert!(error.contains("unsupported value"), "{error}");
    }

    #[test]
    fn mutable_kind_is_rejected() {
        let error = classify_rhai_source("let TEST_KIND = \"graphics\";").unwrap_err();
        assert!(error.contains("top-level const"), "{error}");
    }

    #[test]
    fn computed_kind_cannot_opt_into_graphics() {
        assert_eq!(
            classify_rhai_source("const TEST_KIND = choose_kind();").unwrap(),
            SceneTestKind::Headless
        );
    }

    #[test]
    fn kind_in_a_comment_or_string_is_not_a_declaration() {
        assert_eq!(
            classify_rhai_source(
                r#"// const TEST_KIND = \"graphics\";
                   let text = `const TEST_KIND = \"graphics\";`;"#
            )
            .unwrap(),
            SceneTestKind::Headless
        );
    }

    #[test]
    fn scene_paths_include_nested_isolated_scenes() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("editor/preview_domain_isolation");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.path().join("headless.usda"), "").unwrap();
        fs::write(nested.join("preview_domain_isolation.usda"), "").unwrap();
        fs::write(nested.join("notes.txt"), "").unwrap();

        let mut found = scene_paths(root.path())
            .unwrap()
            .into_iter()
            .map(|path| path.strip_prefix(root.path()).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        found.sort();

        assert_eq!(
            found,
            [
                std::path::PathBuf::from(
                    "editor/preview_domain_isolation/preview_domain_isolation.usda"
                ),
                std::path::PathBuf::from("headless.usda"),
            ]
        );
    }
}
