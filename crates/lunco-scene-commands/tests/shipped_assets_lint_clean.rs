//! Every shipped asset must pass the USD lint rules — the gate that keeps the
//! motor bug from coming back in a different file.
//!
//! The motors fell off every rover because a component asset applied
//! `PhysicsRigidBodyAPI` while nothing jointed it. That is now a rule
//! (`nested-body-no-joint`, `assets/scripting/policy/lint_usd.rhai`). A rule with
//! nothing running it is a comment, so this test runs it over EVERY vessel,
//! component and scene we ship, through the same `ValidateAsset` entry point a
//! human gets from `sandbox --validate`.
//!
//! It registers the policy itself rather than booting an app: the rules are an
//! asset, the hook registry is global, and a `cargo test` that needs a window is
//! a `cargo test` nobody runs.
//!
//! `scenes/tests/lint_selftest.usda` is EXCLUDED by name — it is authored wrong
//! on purpose so `scenarios/tests/lint_selftest.rhai` can prove the rules fire.

use std::path::{Path, PathBuf};

fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Register `lint.usd` from the shipped policy — the same source
/// `lunco_scripting::register_builtin_policies` loads at startup.
fn register_usd_lint_policy() {
    let src = std::fs::read_to_string(assets_dir().join("scripting/policy/lint_usd.rhai"))
        .expect("assets/scripting/policy/lint_usd.rhai is missing");
    lunco_hooks_rhai::register_rhai_hook("lint.usd", "lint_usd", &src, false)
        .expect("the shipped USD lint policy must compile");
}

/// Every `.usda` under `dir`, recursively.
fn usda_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // `.lunco/` holds RUNTIME layers written by the app, not authored
            // assets — linting a machine's scratch state proves nothing.
            if p.file_name().is_some_and(|n| n == ".lunco") {
                continue;
            }
            usda_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "usda") {
            out.push(p);
        }
    }
}

#[test]
fn shipped_usd_assets_have_no_lint_errors() {
    register_usd_lint_policy();

    // COMPLETE assets only. `assets/components/` holds parts and composition
    // OVERLAYS (`physical_drivetrain.usda` is nothing but an articulation root and
    // joints targeting wheels that must already exist), and a rule asked about a fragment in
    // isolation answers a question the file cannot be responsible for: its joint
    // targets, its host body and half its prims arrive with the reference arc.
    // Components are covered where they actually run — every vessel below
    // composes them, and a broken part fails through its hosts.
    let assets = assets_dir();
    let mut files = Vec::new();
    for sub in ["vessels", "scenes", "missions", "tutorials"] {
        usda_files(&assets.join(sub), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 20,
        "expected the shipped asset tree, found {} files",
        files.len()
    );

    let mut offenders = Vec::new();
    for f in &files {
        if f.file_name().is_some_and(|n| n == "lint_selftest.usda") {
            continue;
        }
        let report = lunco_scene_commands::validate::validate_asset(&f.to_string_lossy());
        // Only the LINT findings: a pre-existing parse/compose failure in some
        // unrelated asset is a different test's business, and mixing them would
        // make this one unfixable.
        for e in report.errors.iter().filter(|e| e.starts_with("[usd/")) {
            offenders.push(format!("{}\n    {e}", f.display()));
        }
    }

    assert!(
        offenders.is_empty(),
        "shipped assets with USD lint ERRORS ({}):\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

/// The gate above has teeth.
///
/// "All assets clean" and "the rules never ran" are the same green square, and
/// the second one is how a linter dies quietly. So the deliberately broken scene
/// — the one the clean sweep skips — must come back dirty through the very same
/// `ValidateAsset` path, with the rule that caught the motors.
#[test]
fn the_deliberately_broken_scene_still_fails_the_same_gate() {
    register_usd_lint_policy();

    let broken = assets_dir().join("scenes/tests/lint_selftest.usda");
    let report = lunco_scene_commands::validate::validate_asset(&broken.to_string_lossy());
    let lint_errors: Vec<&String> = report
        .errors
        .iter()
        .filter(|e| e.starts_with("[usd/"))
        .collect();

    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("nested-body-no-joint")),
        "lint_selftest.usda must trip nested-body-no-joint through ValidateAsset — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("empty-component-network")),
        "lint_selftest.usda must prove empty domain networks are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("disconnected-component-network")),
        "lint_selftest.usda must prove disconnected domain networks are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("dangling-network-connector")),
        "lint_selftest.usda must prove out-of-network connectors are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("invalid-network-program-source")),
        "lint_selftest.usda must prove non-Modelica program members are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("multi-source-modelica-property")),
        "lint_selftest.usda must prove scalar Modelica fan-in is rejected — \
         got {lint_errors:?}"
    );
    assert!(!report.ok, "a file with lint ERRORS must not report ok");
}

/// The GEOMETRIC rule has teeth too, and it needs its own case.
///
/// Every other rule reads schemas, ancestry or joint targets — topology, which is
/// what a validator normally sees. `sprung-foot-not-lowest` reads composed
/// transforms and collider extents instead, so it is the one rule that could
/// silently never fire while every fact it depends on quietly returns "unknown".
/// The selftest leg is authored with the descent lander's original geometry: a
/// footpad centred on the strut's tip, clearing its rotated corner by millimetres.
/// Schema-wise it is impeccable.
#[test]
fn a_strut_that_outreaches_its_foot_is_caught_by_geometry_alone() {
    register_usd_lint_policy();

    let broken = assets_dir().join("scenes/tests/lint_selftest.usda");
    let report = lunco_scene_commands::validate::validate_asset(&broken.to_string_lossy());
    let lint_errors: Vec<&String> = report
        .errors
        .iter()
        .filter(|e| e.starts_with("[usd/"))
        .collect();

    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("sprung-foot-thin-clearance")),
        "the selftest leg must trip sprung-foot-thin-clearance — got {lint_errors:?}"
    );
}
