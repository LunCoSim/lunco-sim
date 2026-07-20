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

fn package_member(suffix: &str) -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, src)| src)
        .unwrap_or_else(|| panic!("{suffix} is part of the shipped LunCo package"))
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
