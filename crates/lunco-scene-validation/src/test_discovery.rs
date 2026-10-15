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

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use lunco_usd_bevy_core::program;
#[cfg(not(target_arch = "wasm32"))]
use lunco_usd_bevy_stage::{UsdReadObject, UsdStageProjectionPlan};

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
#[cfg(not(target_arch = "wasm32"))]
pub fn discover_scene_tests(scenes_dir: &Path) -> Result<Vec<SceneTest>, String> {
    let mut scenes = scene_paths(scenes_dir)?;
    scenes.sort();

    // One isolated admission scope per catalogue. Reuse each owning mount
    // without rescanning a folder for every scene that it contains.
    let roots = lunco_assets_core::TwinRoots::default();
    let mut discovered = Vec::with_capacity(scenes.len());
    for scene_path in scenes {
        if let Some(test) = discover_scene_test(&scene_path, &roots)? {
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
pub fn discover_scene_tests(_scenes_dir: &Path) -> Result<Vec<SceneTest>, String> {
    Err("scene-test discovery requires the native asset catalog".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn discover_scene_test(
    scene_path: &Path,
    roots: &lunco_assets_core::TwinRoots,
) -> Result<Option<SceneTest>, String> {
    let prepare = || -> anyhow::Result<_> {
        let absolute = lunco_storage::canonicalize_file_path(scene_path)?;
        let twin_root = lunco_twin::root_for_file(&absolute);
        let authority = match roots.name_for_root(&twin_root)? {
            Some(authority) => authority,
            None => {
                let twin = match lunco_twin::TwinMode::open(&twin_root)? {
                    lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
                    lunco_twin::TwinMode::Orphan(_) => anyhow::bail!("scene owner is not a folder"),
                };
                roots.register_twin(&twin)?
            }
        };
        let origin = bevy::asset::AssetPath::from_path_buf(
            Path::new(&authority).join(absolute.strip_prefix(&twin_root)?),
        )
        .with_source(lunco_assets_core::TWIN_SCHEME);
        let assets_root = lunco_assets_core::shipped_asset_root(&absolute).map(Path::to_path_buf);
        let recipe = lunco_usd_compose::recipe_from_bytes_with_roots(
            &lunco_assets_core::asset_path::anchor_of(&origin),
            lunco_assets_core::read_asset_file_bytes(&absolute)?,
            assets_root.as_deref(),
            Some(&twin_root),
            Default::default(),
        )?;
        let mut view = UsdStageProjectionPlan::from_recipe(&recipe)?;
        view.prepare_native_asset_paths(Some(origin.clone()), Some(roots));
        Ok((view, origin, assets_root))
    };
    let (view, origin, assets_root) = prepare()
        .map_err(|error| format!("{}: cannot prepare scene: {error}", scene_path.display()))?;
    classify_scene_programs(&view, &origin, roots, assets_root.as_deref(), scene_path).map(|kind| {
        kind.map(|kind| SceneTest {
            scene_path: scene_path.to_path_buf(),
            kind,
        })
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn classify_scene_programs(
    view: &dyn UsdReadObject,
    origin: &bevy::asset::AssetPath<'_>,
    roots: &lunco_assets_core::TwinRoots,
    assets_root: Option<&Path>,
    scene_path: &Path,
) -> Result<Option<SceneTestKind>, String> {
    let mut kinds = BTreeSet::new();
    let mut sources = BTreeSet::new();

    for prim in view.prim_paths() {
        if !view.has_api_schema(&prim, "LunCoProgramAPI") {
            continue;
        }
        let resolved = match program::resolve_program(view, &prim) {
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
        let source_path = lunco_assets_core::asset_path::load_asset_path(
            &source_asset,
            Some(origin),
            Some(roots),
            view.native_asset_paths(),
        )
        .map_err(|error| format!("{scene_path:?}: invalid test source {source_asset}: {error}"))?;
        let source_id = lunco_assets_core::asset_path::anchor_of(&source_path);
        let source_root = lunco_assets_core::parse_twin_uri(&source_id)
            .map(|(authority, _)| {
                roots
                    .root_for(authority)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("test source {source_id} has no owning Twin"))
            })
            .transpose()?;
        // Test classification belongs to the catalogue, not to mount-local
        // authority spelling. A nested fixture Twin still lives below tests/.
        // This path is only a classification key; the asset owner reads bytes.
        let classification_path =
            match (&source_root, lunco_assets_core::parse_twin_uri(&source_id)) {
                (Some(root), Some((_, relative))) => {
                    lunco_assets_path::slashed(&root.join(relative))
                }
                _ => lunco_assets_core::engine_asset_rel(&source_id).to_owned(),
            };
        if !lunco_assets_runtime::discovery::is_test_asset(&classification_path)
            || !sources.insert(source_id.clone())
        {
            continue;
        }
        let bytes = lunco_assets_core::read_asset_bytes_with_twin_root(
            &source_id,
            assets_root,
            source_root.as_deref(),
        )
        .map_err(|error| {
            format!(
                "{}: cannot read test source {}: {error}",
                scene_path.display(),
                source_id
            )
        })?;
        let source = String::from_utf8(bytes)
            .map_err(|error| format!("{scene_path:?}: invalid test source {source_id}: {error}"))?;
        let kind = classify_rhai_source(&source).map_err(|error| {
            format!(
                "{}: invalid test declaration in {}: {error}",
                scene_path.display(),
                source_id
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

    Ok(Some(kind))
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use std::fs;
    #[cfg(not(target_arch = "wasm32"))]
    use std::path::Path;

    #[cfg(not(target_arch = "wasm32"))]
    use super::scene_paths;
    use super::{SceneTestKind, classify_rhai_source};

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn composed_test_sources_use_prepared_asset_context() {
        use lunco_usd_bevy_stage::UsdStageProjectionPlan;

        let root = tempfile::tempdir().unwrap();
        let twin_root = root.path().join("tests/fixture");
        lunco_storage::write_file_sync(
            &twin_root.join("scripts/probe.rhai"),
            b"const TEST_KIND = \"graphics\";",
        )
        .unwrap();
        lunco_storage::write_file_sync(
            &twin_root.join("scripts/precedence.rhai"),
            b"const TEST_KIND = \"editor\";",
        )
        .unwrap();
        lunco_storage::write_file_sync(
            &twin_root.join("sim/scenes/scripts/precedence.rhai"),
            b"const TEST_KIND = \"graphics\";",
        )
        .unwrap();
        let roots = lunco_assets_core::TwinRoots::default();
        let authority = roots.register("fixture", &twin_root).unwrap();
        let id = lunco_assets_core::twin_uri(&authority, "sim/scenes/root.usda");
        let origin =
            lunco_assets_core::asset_path::load_asset_path(&id, None, Some(&roots), None).unwrap();
        // Inline layers exercise the composed reader/asset boundary without
        // creating authored USD files or depending on repository assets.
        let classify = |source: &str| {
            let layer = format!(
                r#"#usda 1.0
def Scope "Program" (prepend apiSchemas = ["LunCoProgramAPI"])
{{
    uniform token info:implementationSource = "sourceAsset"
    uniform asset info:sourceAsset = @{source}@
}}
"#
            );
            let recipe = lunco_usd_compose::recipe::StageRecipe::new(
                &id,
                std::collections::HashMap::from([(id.clone(), layer.into_bytes())]),
            );
            let mut plan = UsdStageProjectionPlan::from_recipe(&recipe)
                .map_err(|error| format!("cannot prepare scene: {error}"))?;
            plan.prepare_native_asset_paths(Some(origin.clone()), Some(&roots));
            super::classify_scene_programs(&plan, &origin, &roots, None, Path::new("root.usda"))
        };
        assert_eq!(
            classify("scripts/probe.rhai").unwrap(),
            Some(SceneTestKind::Graphics)
        );
        assert_eq!(
            classify("scripts/precedence.rhai").unwrap(),
            Some(SceneTestKind::Graphics)
        );
        assert_eq!(
            classify("./scripts/precedence.rhai").unwrap(),
            Some(SceneTestKind::Graphics)
        );
        let error = classify("./scripts/probe.rhai").unwrap_err();
        assert!(error.contains("cannot read test source"), "{error}");
        assert!(error.contains("sim/scenes/scripts/probe.rhai"), "{error}");
        let error = classify("scripts/absent.rhai").unwrap_err();
        assert!(error.contains("cannot read test source"), "{error}");
        let error = classify("file:///tests/invalid%00.rhai").unwrap_err();
        assert!(error.contains("file:///tests/invalid%00.rhai"), "{error}");
        assert!(error.contains("invalid file URI"), "{error}");

        lunco_storage::write_file_sync(
            &twin_root.join("sim/scenes/parts/scripts/child.rhai"),
            b"const TEST_KIND = \"graphics\";",
        )
        .unwrap();
        lunco_storage::write_file_sync(
            &twin_root.join("sim/scenes/scripts/child.rhai"),
            b"const TEST_KIND = \"editor\";",
        )
        .unwrap();
        let child_id = lunco_assets_core::twin_uri(&authority, "sim/scenes/parts/source.usda");
        let recipe = lunco_usd_compose::recipe::StageRecipe::new(
            &id,
            std::collections::HashMap::from([
                (
                    id.clone(),
                    br#"#usda 1.0
def Scope "Program" (prepend references = @parts/source.usda@</Program>) {}
"#
                    .to_vec(),
                ),
                (
                    child_id,
                    br#"#usda 1.0
def Scope "Program" (prepend apiSchemas = ["LunCoProgramAPI"])
{
    uniform token info:implementationSource = "sourceAsset"
    uniform asset info:sourceAsset = @scripts/child.rhai@
}
"#
                    .to_vec(),
                ),
            ]),
        );
        let mut plan = UsdStageProjectionPlan::from_recipe(&recipe).unwrap();
        plan.prepare_native_asset_paths(Some(origin.clone()), Some(&roots));
        assert_eq!(
            super::classify_scene_programs(&plan, &origin, &roots, None, Path::new("root.usda"))
                .unwrap(),
            Some(SceneTestKind::Graphics)
        );
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

    #[cfg(not(target_arch = "wasm32"))]
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
