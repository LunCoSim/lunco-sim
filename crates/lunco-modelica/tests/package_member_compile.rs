//! Compiling a `.mo` that is a MEMBER of a shipped Modelica package.
//!
//! A file beginning `within LunCo.Propulsion;` does not stand on its own: its
//! class is `LunCo.Propulsion.BellNozzle`, and the shipped `LunCo` source root
//! already owns a copy of it. Seating such a file as a standalone user document
//! registers that qualified class a second time, and rumoca's merge pass rejects
//! the pair with `Duplicate class '…' with non-identical definition`.
//!
//! This is not hypothetical: three USD prims point `info:sourceAsset`
//! at package members (the descent lander's `BellNozzle`, and `SunTracker` in
//! two sandbox scenes). Every one of them failed to solve while its geometry
//! kept rendering — the lathe is Rust-side, so the failure was invisible on
//! screen. These tests pin the resolution so it cannot regress into silence.

use lunco_modelica::ModelicaCompiler;
use rumoca_compile::Session;
use rumoca_sim::{SimOptions, SimulationSession};

fn package_member(suffix: &str) -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, src)| src)
        .unwrap_or_else(|| panic!("{suffix} is part of the shipped LunCo package"))
}

#[test]
fn lunco_logic_class_is_registered_in_a_rumoca_session() {
    let mut session = Session::default();
    let files = lunco_assets::models::package_files("LunCo");
    assert!(!files.is_empty());
    for (uri, source) in files {
        session
            .add_document(&uri, &source)
            .unwrap_or_else(|error| panic!("{uri}: {error}"));
    }
    for qualified in [
        "LunCo.Propulsion.BellNozzle",
        "LunCo.Pointing.SunTracker",
        "LunCo.Logic.AboveThreshold",
    ] {
        eprintln!("{qualified}: {:?}", session.class_lookup_query(qualified));
    }
    assert_eq!(
        session.class_lookup_query("LunCo.Logic.AboveThreshold"),
        Some("LunCo.Logic.AboveThreshold".to_string())
    );
}

/// The regression itself: a package member compiles, by the short name the USD
/// cosim dispatcher passes, without tripping the duplicate-class merge error.
#[test]
fn package_member_compiles_without_duplicate_class() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "BellNozzle",
        &package_member("Propulsion/BellNozzle.mo"),
        "lunco://models/LunCo/Propulsion/BellNozzle.mo",
    );
    let err = result.err();
    assert!(
        err.is_none(),
        "BellNozzle should compile as a package member, got: {err:?}"
    );
}

/// The failure mode was silent, so "no error" is not enough to assert: the model
/// has to actually SOLVE. These are the outputs the film reads off the nozzle.
#[test]
fn package_member_publishes_its_outputs() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "BellNozzle",
            &package_member("Propulsion/BellNozzle.mo"),
            "lunco://models/LunCo/Propulsion/BellNozzle.mo",
        )
        .expect("BellNozzle compiles");

    // The nozzle publishes its engineering as `output` declarations, so they land
    // in the DAE's `w` partition; algebraics are checked too so a future
    // re-partitioning of the same quantity does not read as a regression.
    let names: Vec<String> = dae
        .dae
        .variables
        .outputs
        .iter()
        .chain(dae.dae.variables.algebraics.iter())
        .map(|(name, _)| name.to_string())
        .collect();
    for expected in ["expansion_ratio", "cf", "isp_vac", "thrust"] {
        assert!(
            names.iter().any(|n| n == expected || n.ends_with(expected)),
            "`{expected}` should be a live output; solved variables = {names:?}"
        );
    }
}

/// The same routing must hold for the other authored package member, so the fix
/// is not one model deep.
#[test]
fn sun_tracker_package_member_compiles() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "SunTracker",
        &package_member("Pointing/SunTracker.mo"),
        "lunco://models/LunCo/Pointing/SunTracker.mo",
    );
    assert!(result.is_ok(), "SunTracker: {:?}", result.err());
}

/// The tracker is used through USD wires, so compilation alone is not its
/// contract.  The solver must retain the public sun-vector inputs and change
/// its yaw output after a new vector arrives.  This pins the Modelica half of
/// the USD → Modelica → joint chain independently of Bevy port propagation.
#[test]
fn sun_tracker_reacts_to_a_runtime_sun_vector() {
    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "SunTracker",
            &package_member("Pointing/SunTracker.mo"),
            "lunco://models/LunCo/Pointing/SunTracker.mo",
        )
        .expect("SunTracker compiles");

    let mut opts = SimOptions::default();
    opts.atol = 1e-3;
    opts.rtol = 1e-3;
    // `SimulationSession::default()` ends at one second.  This regression
    // deliberately performs a post-settle input step, so it needs the same
    // non-trivial horizon that the live co-simulation session uses.
    opts.t_end = 10.0;
    let mut stepper = SimulationSession::new(&dae.dae, opts).expect("SunTracker stepper builds");

    for input in ["sun_mount_x", "sun_mount_y", "sun_mount_z"] {
        assert!(
            stepper.input_names().iter().any(|name| name == input),
            "SunTracker must expose `{input}` as a runtime input, got {:?}",
            stepper.input_names()
        );
    }

    stepper
        .set_input("sun_mount_x", 0.0)
        .expect("set initial x");
    stepper
        .set_input("sun_mount_y", 0.0)
        .expect("set initial y");
    stepper
        .set_input("sun_mount_z", -1.0)
        .expect("set initial z");
    for _ in 0..240 {
        stepper.step(1.0 / 60.0).expect("settle initial yaw");
    }
    let yaw_before = stepper
        .get("yaw")
        .expect("read initial yaw")
        .expect("yaw is observable");

    stepper.set_input("sun_mount_x", 1.0).expect("step x");
    for _ in 0..240 {
        stepper.step(1.0 / 60.0).expect("settle stepped yaw");
    }
    let yaw_after = stepper
        .get("yaw")
        .expect("read stepped yaw")
        .expect("yaw is observable");

    assert!(
        (yaw_after - yaw_before).abs() > 0.2,
        "SunTracker ignored its updated sun vector: yaw {yaw_before} -> {yaw_after}"
    );
}

#[test]
fn earth_tracker_package_member_compiles() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "EarthTracker",
        &package_member("Pointing/EarthTracker.mo"),
        "lunco://models/LunCo/Pointing/EarthTracker.mo",
    );
    assert!(result.is_ok(), "EarthTracker: {:?}", result.err());
}

#[test]
fn nested_logic_package_member_compiles_with_its_inline_icon_base() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "AboveThreshold",
        &package_member("Logic/AboveThreshold.mo"),
        "lunco://models/LunCo/Logic/AboveThreshold.mo",
    );
    assert!(result.is_ok(), "AboveThreshold: {:?}", result.err());
}

#[test]
fn lander_resolves_nested_logic_dependency() {
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(
        "Lander",
        lunco_modelica::models::get_model("Lander.mo").expect("bundled Lander.mo"),
        "lunco://models/Lander.mo",
    );
    assert!(result.is_ok(), "Lander: {:?}", result.err());
}

#[test]
fn stripped_lander_resolves_nested_logic_dependency() {
    let source = lunco_modelica::models::get_model("Lander.mo").expect("bundled Lander.mo");
    let (stripped, _defaults, issues) =
        lunco_modelica::ast_extract::strip_input_defaults_with_report(source);
    assert!(
        issues.is_empty(),
        "Lander preprocessing must not report input-default issues: {issues:?}"
    );

    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str("Lander", &stripped, "lunco://models/Lander.mo");
    assert!(
        result.is_ok(),
        "worker-preprocessed Lander must resolve LunCo.Logic.AboveThreshold: {:?}",
        result.err()
    );
}

#[test]
fn disk_lander_resolves_nested_logic_dependency() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/Lander.mo"),
    )
    .expect("disk Lander.mo");
    let (stripped, _defaults, issues) =
        lunco_modelica::ast_extract::strip_input_defaults_with_report(&source);
    assert!(
        issues.is_empty(),
        "disk Lander preprocessing must not report input-default issues: {issues:?}"
    );

    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str("Lander", &stripped, "lunco://models/Lander.mo");
    assert!(
        result.is_ok(),
        "disk Lander must resolve LunCo.Logic.AboveThreshold: {:?}",
        result.err()
    );
}

/// The library must be seated from the DISK tree, not the `include_dir!` snapshot
/// baked into the binary.
///
/// Both copies exist and they drift the moment a `.mo` is edited without a
/// rebuild. The disk tree is what Bevy's AssetServer serves, so it is already
/// what `info:sourceAsset` reads; if the library came from the embedded
/// snapshot instead, an edited member would compile as its last-BUILT self while
/// the scene had loaded the new text — the two disagreeing in silence.
///
/// The scope this pins is edits that are SAVED. A library class is resolved by
/// name out of its seated root, so an unsaved editor buffer does not reach the
/// compiler, and a root already seated in a session is not re-read when the file
/// changes underneath it. That is what Modelica does — a loaded library is
/// reloaded, not patched per-compile — and it is strictly better than what came
/// before, which was that a package member did not compile at all.
/// Anchored on `CARGO_MANIFEST_DIR`, not the process CWD. `models_package_root_path`
/// resolves against `assets_dir_abs()`, which is CWD-joined by design — correct for
/// the running binary (whose CWD is the workspace root, where the AssetServer is
/// pointed) but NOT for a test harness, whose CWD is the crate directory. Asserting
/// on the function's return here would pass or fail on where cargo happened to stand.
#[test]
fn library_root_is_the_live_disk_tree() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<crate> sits two levels under the workspace root")
        .to_path_buf();
    let root = workspace.join("assets/models/LunCo");

    assert!(
        root.join("package.mo").is_file(),
        "{} must be a STRUCTURED entity — package.mo is what makes the directory a \
         Modelica package rather than a folder of .mo files",
        root.display()
    );
    assert!(
        root.join("Propulsion/BellNozzle.mo").is_file(),
        "the member the film compiles must live under the library root, so that \
         seating the root is what makes it resolvable"
    );

    // The contract itself: whenever the path resolves, it is the AssetServer's tree
    // and not some other `models/LunCo` — that parity is what keeps a saved edit and
    // the compiled class the same file.
    if let Some(resolved) = lunco_assets::models_package_root_path("LunCo") {
        assert!(
            resolved.ends_with("assets/models/LunCo"),
            "resolved root must be the AssetServer's tree, got {}",
            resolved.display()
        );
        assert!(resolved.join("package.mo").is_file());
    }
}

/// A standalone `.mo` (no `within`) keeps the ordinary user-overlay path — the
/// package-member routing must not swallow the common case.
#[test]
fn standalone_model_still_compiles_via_user_overlay() {
    let balloon = lunco_modelica::models::get_model("Balloon.mo").expect("bundled Balloon.mo");
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str("Balloon", balloon, "balloon.mo");
    assert!(result.is_ok(), "Balloon: {:?}", result.err());
}
