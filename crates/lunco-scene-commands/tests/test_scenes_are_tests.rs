//! A scene called `*_test.usda` must actually be a test.
//!
//! Four of them were not. `differential_rig_test`, `rocker_bogie_test`,
//! `g7_joints_test`, `prismatic_drive_test` and `revolute_motor_test` carried no
//! `LunCoProgram` at all, so `scene_test` ran them for 20000 ticks, received no
//! verdict, and exited 2 — every time, for as long as they had existed. Their
//! invariants were real and written down; they were written down as instructions
//! to a HUMAN, in the file header:
//!
//! ```text
//! # Verify (ListPorts): HingeR.angle ≈ −HingeL.angle (rocker B mirrors A).
//! ```
//!
//! That is the failure this guards: not a broken test, but a file that looks like
//! a test, sits with the tests, is named like a test, and asserts nothing. Nobody
//! notices, because nothing is red.
//!
//! Scenes that are legitimately NOT automatable are listed below BY NAME with a
//! reason. An explicit exception is a decision; a silent gap is a bug.

use std::path::{Path, PathBuf};

fn scenes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenes/sandbox")
}

/// Scenes whose name ends in `_test` that cannot carry a headless verdict.
///
/// Each needs a reason, and the reason must be about the SCENE, not about the
/// effort of writing one — "it would be work" is how this list becomes the place
/// tests go to die.
const NOT_HEADLESS_TESTABLE: &[(&str, &str)] = &[
    ("hdri_test", "a render check: the thing under test is the image, and scene_test has no GPU"),
    ("_shader_fallback_test", "a render check: asserts what a material looks like when its shader is missing"),
];

#[test]
fn every_scene_named_test_carries_a_scenario() {
    let dir = scenes_dir();
    let mut checked = 0;
    let mut silent = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("assets/scenes/sandbox is missing").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "usda") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        if !stem.ends_with("_test") {
            continue;
        }
        checked += 1;
        if NOT_HEADLESS_TESTABLE.iter().any(|(n, _)| *n == stem) {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read scene");
        // A verdict needs a scenario to emit it, and a scenario reaches the scene
        // through a `LunCoProgram`. Both, so that neither half can rot alone.
        if !src.contains("lunco:scenario") || !src.contains("LunCoProgram") {
            silent.push(stem);
        }
    }

    assert!(checked > 10, "expected the sandbox test scenes, found {checked}");
    assert!(
        silent.is_empty(),
        "scenes named `_test` that assert nothing ({}):\n  {}\n\n\
         Give each one a scenario that checks the invariant its header already \
         describes, or add it to NOT_HEADLESS_TESTABLE with a reason. A scene \
         that cannot fail is not a test.",
        silent.len(),
        silent.join("\n  ")
    );
}
