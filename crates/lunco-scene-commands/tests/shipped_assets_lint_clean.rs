//! Every shipped asset must pass the USD lint rules — the gate that keeps the
//! motor bug from coming back in a different file.
//!
//! The motors fell off every rover because a component asset applied
//! `PhysicsRigidBodyAPI` while nothing jointed it. That is now a rule
//! (`nested-body-no-joint`, `assets/scripting/policy/lint_usd.rhai`). A rule with
//! nothing running it is a comment, so this test runs it over EVERY vessel,
//! component and scene we ship, through the same `ValidateAsset` entry point a
//! human gets from `luncosim --validate`.
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

    // NO findings and NO rules is the same empty list, and the difference is
    // always in the errors this filter drops — a scene that failed to compose, or
    // a policy that failed to compile, reports there. Saying so here is the
    // difference between a two-minute fix and an afternoon.
    assert!(
        !lint_errors.is_empty(),
        "the deliberately broken scene produced NO lint findings at all — the rules \
         did not run. Full report: ok={} errors={:?} warnings={:?}",
        report.ok,
        report.errors,
        report.warnings,
    );
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
            .any(|e| e.contains("conditionally-stable-joint-drive")
                && e.contains("/LintSelftest/Leg/Strut_Spring")),
        "lint_selftest.usda must reject the un-massed conditional drive through the same ValidateAsset path — got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("joint-drive-negative-stiffness")
                && e.contains("/LintSelftest/Leg/Strut_NegativeStiffness")),
        "lint_selftest.usda must reject negative stiffness with its dedicated finding — got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("invalid-gear-drive") && e.contains("/LintSelftest/BadGear")),
        "lint_selftest.usda must reject malformed gear-drive authoring through the same ValidateAsset path — got {lint_errors:?}"
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
            .any(|e| e.contains("connector-requires-modelica")),
        "lint_selftest.usda must prove non-Modelica connectors are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("connector-requires-network-root")),
        "lint_selftest.usda must prove standalone connectors are rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("invalid-modelica-causal-cardinality")),
        "lint_selftest.usda must prove scalar Modelica fan-in is rejected — \
         got {lint_errors:?}"
    );
    assert!(
        lint_errors
            .iter()
            .any(|e| e.contains("ambiguous-modelica-boundary")),
        "lint_selftest.usda must prove composed boundary aliasing is rejected — \
         got {lint_errors:?}"
    );
    assert!(!report.ok, "a file with lint ERRORS must not report ok");
}

#[test]
fn targetless_metadata_telemetry_is_a_usd_lint_error() {
    register_usd_lint_policy();
    let path = assets_dir().join("scenes/tests/lint_selftest.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose lint fixture");
    let canonical =
        lunco_usd_bevy::CanonicalStage::from_stage(stage, path.to_string_lossy().into_owned());
    let findings = lunco_scene_commands::lint_command::lint_stage(&canonical.view());
    assert!(
        findings.iter().any(|finding| {
            finding.rule == "telemetry-target-required"
                && finding.subject == "/LintSelftest/BrokenTelemetry"
        }),
        "targetless metadata telemetry must be visible in the live USD lint findings: {findings:?}"
    );
}

#[test]
fn massed_linear_force_drives_are_not_false_positive_lint_errors() {
    register_usd_lint_policy();
    let asset = assets_dir().join("scenes/tests/prismatic_spring.usda");
    let report = lunco_scene_commands::validate::validate_asset(&asset.to_string_lossy());
    let conditional: Vec<&String> = report
        .errors
        .iter()
        .filter(|e| e.contains("conditionally-stable-joint-drive"))
        .collect();
    assert!(
        conditional.is_empty(),
        "massed linear force drives must resolve to the implicit SpringDamper path, not lint errors: {conditional:?}"
    );
}

#[test]
fn documented_pure_force_dampers_are_not_false_positive_lint_errors() {
    use lunco_hooks::HookValue as H;

    register_usd_lint_policy();
    let empty = || H::Array(Vec::new());
    let facts = H::map([
        (
            "stage",
            H::map([
                ("meters_per_unit_authored", H::Bool(true)),
                ("fixed_hz", H::Float(60.0)),
                (
                    "physics_substeps",
                    H::Int(lunco_physics::DEFAULT_SUBSTEP_COUNT as i64),
                ),
                (
                    "substep_dt",
                    H::Float(
                        1.0 / (lunco_core::FIXED_HZ * lunco_physics::DEFAULT_SUBSTEP_COUNT as f64),
                    ),
                ),
            ]),
        ),
        ("bodies", empty()),
        ("joints", empty()),
        ("prims", empty()),
        ("collections", empty()),
        ("filtered_pairs", empty()),
        ("collision_groups", empty()),
        ("network_roots", empty()),
        ("collision_enabled_without_api", empty()),
        ("unsupported_program_prims", empty()),
        ("connector_programs", empty()),
        (
            "drives",
            H::Array(vec![H::map([
                ("path", H::str("/Rig/Damper")),
                ("realization", H::str("force_based")),
                ("stiffness", H::Float(0.0)),
                ("damping", H::Float(10.0)),
                ("damping_ratio", H::Float(0.0)),
            ])]),
        ),
        ("gear_drives", empty()),
        ("wheel_attachments", empty()),
        ("invalid_wheel_attachments", empty()),
        ("passive_suspensions", empty()),
    ]);

    let findings = lunco_lint::run_lint("usd", facts);
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "conditionally-stable-joint-drive"),
        "the documented positive pure ForceBased damper must not be rejected: {findings:?}"
    );
}

#[test]
fn a_collection_query_failure_is_not_misreported_as_an_empty_network() {
    use lunco_hooks::HookValue as H;

    register_usd_lint_policy();
    let empty = || H::Array(Vec::new());
    let scope = H::map([
        ("path", H::str("/BrokenNetwork")),
        ("parent", H::str("/")),
        ("members", empty()),
        ("synthesizer", H::str("acausal-network")),
        ("synthesizer_error", H::str("")),
        ("modelica_member_count", H::Int(0)),
        (
            "collection_error",
            H::str("OpenUSD membership query failed"),
        ),
        ("units", empty()),
        ("dangling_connectors", empty()),
        ("invalid_program_sources", empty()),
        ("invalid_causal_properties", empty()),
        ("ambiguous_boundary_sources", empty()),
    ]);
    let facts = H::map([
        (
            "stage",
            H::map([
                ("meters_per_unit_authored", H::Bool(true)),
                ("fixed_hz", H::Float(60.0)),
                (
                    "physics_substeps",
                    H::Int(lunco_physics::DEFAULT_SUBSTEP_COUNT as i64),
                ),
                (
                    "substep_dt",
                    H::Float(
                        1.0 / (lunco_core::FIXED_HZ * lunco_physics::DEFAULT_SUBSTEP_COUNT as f64),
                    ),
                ),
            ]),
        ),
        ("bodies", empty()),
        ("joints", empty()),
        ("prims", empty()),
        ("collections", empty()),
        ("filtered_pairs", empty()),
        ("collision_groups", empty()),
        ("network_roots", H::Array(vec![scope])),
        ("collision_enabled_without_api", empty()),
        // The WHOLE fact table, including the keys this case has nothing to say
        // about. `physics_facts` always emits every key, and a rule reading one
        // that is absent aborts `lint_usd` — taking every OTHER rule down with it,
        // and reporting as an empty findings list. A fixture that omits keys is
        // testing a fact table we do not ship.
        ("unsupported_program_prims", empty()),
        ("connector_programs", empty()),
        ("telemetry_declarations", empty()),
        ("drives", empty()),
        ("gear_drives", empty()),
        ("wheel_attachments", empty()),
        ("invalid_wheel_attachments", empty()),
        ("passive_suspensions", empty()),
    ]);

    let findings = lunco_lint::run_lint("usd", facts);
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "invalid-component-collection"),
        "the OpenUSD error must survive into policy findings: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "empty-component-network"),
        "a failed membership query is not evidence of an empty collection: {findings:?}"
    );
}

#[test]
fn omitted_stage_units_are_highlighted_without_rejecting_valid_usd() {
    use lunco_hooks::HookValue as H;

    register_usd_lint_policy();
    let empty = || H::Array(Vec::new());
    let facts = H::map([
        (
            "stage",
            H::map([
                ("meters_per_unit_authored", H::Bool(false)),
                ("fixed_hz", H::Float(60.0)),
                (
                    "physics_substeps",
                    H::Int(lunco_physics::DEFAULT_SUBSTEP_COUNT as i64),
                ),
                (
                    "substep_dt",
                    H::Float(
                        1.0 / (lunco_core::FIXED_HZ * lunco_physics::DEFAULT_SUBSTEP_COUNT as f64),
                    ),
                ),
            ]),
        ),
        ("bodies", empty()),
        ("joints", empty()),
        ("prims", empty()),
        ("collections", empty()),
        ("filtered_pairs", empty()),
        ("collision_groups", empty()),
        ("network_roots", empty()),
        ("collision_enabled_without_api", empty()),
        ("unsupported_program_prims", empty()),
        ("connector_programs", empty()),
        ("telemetry_declarations", empty()),
        ("drives", empty()),
        ("gear_drives", empty()),
        ("wheel_attachments", empty()),
        ("invalid_wheel_attachments", empty()),
        ("passive_suspensions", empty()),
    ]);

    let findings = lunco_lint::run_lint("usd", facts);
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "stage-meters-per-unit-missing")
        .expect("an omitted metersPerUnit must be highlighted");
    assert_eq!(finding.severity, lunco_lint::LintSeverity::Warn);
    assert!(
        finding.message.contains("0.01 m/unit"),
        "the warning must explain the OpenUSD fallback: {finding:?}"
    );
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
