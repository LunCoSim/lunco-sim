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
//! Every scene must bind an authored test observer. The observer's Rhai source
//! declares `const TEST_KIND = "graphics"` when it needs the GPU; omission is
//! the deterministic headless default.

use std::path::{Path, PathBuf};

fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn usda_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)
        .expect("read scenes dir")
        .map(|entry| entry.expect("read scene directory entry"))
    {
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
    let discovered = lunco_scene_commands::test_discovery::discover_scene_tests(&dir)
        .expect("every scene must bind a valid test Rhai observer");

    assert!(
        scenes.len() > 10,
        "expected the test scenes, found {}",
        scenes.len()
    );
    assert_eq!(
        discovered.len(),
        scenes.len(),
        "discovery must classify every scene exactly once"
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
