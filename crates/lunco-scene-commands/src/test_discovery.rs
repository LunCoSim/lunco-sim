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
//! ```
//!
//! This lets a runner classify a test without executing user code.  An omitted
//! declaration means `headless`, which keeps existing scenario tests concise
//! while making graphics tests an explicit opt-in.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lunco_usd_bevy::{program, StageView, UsdRead};

/// The Rhai constant read by the test discovery pass.
pub const TEST_KIND_CONST: &str = "TEST_KIND";
/// The default and explicit value for CPU-only deterministic tests.
pub const HEADLESS_TEST_KIND: &str = "headless";
/// The value for tests whose assertion consumes rendered pixels or UI output.
pub const GRAPHICS_TEST_KIND: &str = "graphics";

/// Which runtime a scene test needs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SceneTestKind {
    /// No GPU or window/render world is required.
    Headless,
    /// Run through the offscreen renderer and inspect graphics output.
    Graphics,
}

impl SceneTestKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Headless => HEADLESS_TEST_KIND,
            Self::Graphics => GRAPHICS_TEST_KIND,
        }
    }
}

/// One scene and the execution domain declared by its authored test program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneTest {
    pub scene_path: PathBuf,
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

    let value = value
        .into_string()
        .map_err(|_| format!("`{TEST_KIND_CONST}` must be `\"headless\"` or `\"graphics\"`"))?;
    match value.as_str() {
        HEADLESS_TEST_KIND => Ok(SceneTestKind::Headless),
        GRAPHICS_TEST_KIND => Ok(SceneTestKind::Graphics),
        _ => Err(format!(
            "`{TEST_KIND_CONST}` has unsupported value {value:?}; expected `{HEADLESS_TEST_KIND}` or `{GRAPHICS_TEST_KIND}`"
        )),
    }
}

/// Discover every test scene below `scenes_dir` through composed USD reads.
///
/// Only `.rhai` programs below an asset `tests/` directory are test observers
/// (`scenarios/tests/` in the shipped library). A scene may also carry a
/// production/tutorial Rhai program; those are intentionally ignored here.
/// Multiple test observers must agree on their domain so the runner never has
/// to guess which half of a scene it should execute.
pub fn discover_scene_tests(scenes_dir: &Path) -> Result<Vec<SceneTest>, String> {
    let entries = std::fs::read_dir(scenes_dir)
        .map_err(|error| format!("cannot read {}: {error}", scenes_dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot enumerate {}: {error}", scenes_dir.display()))?;
    let mut scenes = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "usda")
        })
        .collect::<Vec<_>>();
    scenes.sort();

    let mut discovered = Vec::with_capacity(scenes.len());
    for scene_path in scenes {
        discovered.push(discover_scene_test(&scene_path)?);
    }
    Ok(discovered)
}

fn discover_scene_test(scene_path: &Path) -> Result<SceneTest, String> {
    let stage = lunco_usd_bevy::compose_file_to_stage(scene_path)
        .map_err(|error| format!("{}: cannot compose scene: {error}", scene_path.display()))?;
    let view = StageView::new(&stage);
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
        let source_rel = lunco_assets::engine_asset_rel(&source_asset);
        if !lunco_assets::discovery::is_test_asset(&source_rel) || !source_rel.ends_with(".rhai") {
            continue;
        }
        if !sources.insert(source_asset.clone()) {
            continue;
        }

        // Resolve relative to the scene's own shipped-library root rather than
        // the process CWD. Cargo integration tests run with the package as the
        // current directory, while the production binary normally runs from
        // the workspace or beside a packaged `assets/` directory.
        let source_path = lunco_assets::id_to_disk_path(
            &source_asset,
            lunco_assets::shipped_asset_root(scene_path),
        )
        .ok_or_else(|| format!("{scene_path:?}: cannot resolve test source {source_asset}"))?;
        let source = lunco_assets::read_asset_file_string(&source_path).map_err(|error| {
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
        0 => {
            return Err(format!(
                "{}: no test Rhai program bound through LunCoProgramAPI; expected a source below an asset tests/ directory",
                scene_path.display()
            ));
        }
        1 => *kinds.first().expect("one kind exists"),
        _ => {
            return Err(format!(
                "{}: test Rhai programs disagree on TEST_KIND",
                scene_path.display()
            ));
        }
    };

    Ok(SceneTest {
        scene_path: scene_path.to_path_buf(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::{classify_rhai_source, SceneTestKind};

    #[test]
    fn omitted_kind_is_headless() {
        assert_eq!(
            classify_rhai_source("fn on_start(me) { let value = me; }").unwrap(),
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
}
