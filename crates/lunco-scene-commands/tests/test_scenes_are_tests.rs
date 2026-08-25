//! A scene in `assets/scenes/tests/` must actually be a test.
//!
//! Several were not. `differential_rig`, `rocker_bogie`, `g7_joints`,
//! `prismatic_drive` and `revolute_motor` carried no program API at all, so
//! `scene_test` ran them for 20000 ticks, received no verdict, and exited 2 —
//! every time, for as long as they had existed. Their invariants were real and
//! written down; they were written down as instructions to a HUMAN, in the file
//! header:
//!
//! ```text
//! # Verify (ListPorts): HingeR.angle ≈ −HingeL.angle (rocker B mirrors A).
//! ```
//!
//! That is the failure this guards: not a broken test, but a file that looks like
//! a test, sits with the tests, is named like a test, and asserts nothing. Nobody
//! notices, because nothing is red.
//!
//! The second half is the reverse direction: a test scene left OUTSIDE
//! `scenes/tests/` is invisible to `scripts/run_scene_tests.sh`, which discovers
//! by directory. Both checks live here so neither half can rot alone.
//!
//! Scenes that are legitimately NOT automatable are listed below BY NAME with a
//! reason. An explicit exception is a decision; a silent gap is a bug.

use std::path::{Path, PathBuf};

fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// The attribute a scene uses to declare it cannot carry a headless verdict, with
/// the reason as its value.
///
/// It lives in the SCENE, not in a list here, because
/// `scripts/run_scene_tests.sh` needs the same answer — it skips exactly these —
/// and two exception lists disagree the day one of them is edited. The reason
/// must be about the scene (the render checks need a GPU), never about the effort
/// of writing a scenario: "it would be work" is how an exception list becomes the
/// place tests go to die.
const NOT_HEADLESS_TESTABLE: &str = "lunco:notHeadlessTestable";

fn usda_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read scenes dir").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "usda") {
            continue;
        }
        out.push((
            path.file_stem().unwrap().to_string_lossy().to_string(),
            path,
        ));
    }
    out
}

#[test]
fn every_test_scene_carries_a_scenario() {
    let dir = assets_dir().join("scenes/tests");
    let scenes = usda_files(&dir);
    let mut silent = Vec::new();

    for (stem, path) in &scenes {
        let src = std::fs::read_to_string(path).expect("read scene");
        if src.contains(NOT_HEADLESS_TESTABLE) {
            continue;
        }
        // A verdict needs a test scenario to emit it, and a scenario reaches the
        // scene through `LunCoProgramAPI`. Check the actual asset edge rather
        // than a legacy marker that can remain present while the edge is absent.
        let has_test_scenario = src
            .lines()
            .any(|line| line.contains("info:sourceAsset = @lunco://scenarios/tests/"));
        if !has_test_scenario || !src.contains("LunCoProgramAPI") {
            silent.push(stem.clone());
        }
    }

    assert!(
        scenes.len() > 10,
        "expected the test scenes, found {}",
        scenes.len()
    );
    silent.sort();
    assert!(
        silent.is_empty(),
        "test scenes that assert nothing ({}):\n  {}\n\n\
         Give each one a scenario that checks the invariant its header already \
         describes, or — if it genuinely cannot return a headless verdict — author \
         `custom string {NOT_HEADLESS_TESTABLE} = \"<why>\"` on its root prim. A \
         scene that cannot fail is not a test.",
        silent.len(),
        silent.join("\n  ")
    );
}

/// The marker/API check above proves that a scene intends to run a scenario. It
/// does not prove that the USD asset edge resolves: a misspelled `.rhai` URI
/// still contains both strings and would only become a no-verdict runtime
/// failure. Keep this contract at the scene-test owner, and mirror its cheap
/// path check in `scripts/run_scene_tests.sh` so `--no-build` catches the same
/// authoring error before launching the production binary.
#[test]
fn every_headless_test_scene_references_an_existing_test_scenario() {
    let dir = assets_dir().join("scenes/tests");
    let scenario_dir = assets_dir().join("scenarios/tests");
    let mut missing = Vec::new();

    for (stem, path) in usda_files(&dir) {
        let source = std::fs::read_to_string(&path).expect("read scene");
        if source.contains(NOT_HEADLESS_TESTABLE) {
            continue;
        }

        let refs: Vec<String> = source
            .lines()
            .filter_map(|line| {
                let rest = line
                    .split_once("info:sourceAsset = @lunco://scenarios/tests/")?
                    .1;
                let name = rest.split('@').next()?;
                (!name.is_empty()).then(|| name.to_string())
            })
            .collect();

        if refs.is_empty() {
            missing.push(format!("{stem}: no scenarios/tests sourceAsset"));
            continue;
        }

        for scenario in refs {
            if !scenario_dir.join(&scenario).is_file() {
                missing.push(format!("{stem}: missing scenarios/tests/{scenario}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "headless scene→scenario asset edges do not resolve:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn no_test_scene_hides_outside_the_tests_directory() {
    // `scripts/run_scene_tests.sh` discovers by DIRECTORY. A rig written into
    // `scenes/luncosim/` runs in nobody's gate however carefully it asserts, and
    // its name is the only trace that it was ever meant to.
    let stray: Vec<String> = usda_files(&assets_dir().join("scenes/luncosim"))
        .into_iter()
        .filter(|(stem, _)| {
            stem.contains("_test") || stem.contains("parity") || stem.contains("selftest")
        })
        .map(|(stem, _)| stem)
        .collect();

    assert!(
        stray.is_empty(),
        "scene(s) named like tests but living in scenes/luncosim/ ({}):\n  {}\n\n\
         `scripts/run_scene_tests.sh` runs assets/scenes/tests/ — a rig outside it \
         gates nothing. Move it there, or rename it to what it actually is.",
        stray.len(),
        stray.join("\n  ")
    );
}
