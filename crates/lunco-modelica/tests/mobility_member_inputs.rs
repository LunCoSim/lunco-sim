//! The rover drive laws, after their move into `LunCo.Mobility`.
//!
//! `RoverDrivetrain.mo` and `RoverAckermannDrivetrain.mo` used to be bare files
//! in the flat `assets/models/` pile. Moving them into the package gives them a
//! `within LunCo.Mobility;` header, which switches `compile_str` from the
//! document path to the LIBRARY path — the one that skipped
//! `strip_input_defaults` and returned an EMPTY `input_names()`, so the cosim
//! rejected every wire and the model ran on its declared defaults while
//! publishing plausible numbers. See `library_member_inputs.rs` for the full
//! account of that failure.
//!
//! These models take no bound defaults (`input Real throttle "…"`, no `= 0.0`),
//! so they should never have been exposed to that specific bug — but "should
//! not be" is the wrong standard for a change that silently produces a working-
//! looking rover. The relocation is checked here directly, because **no scene
//! test covers it**: `driveLaw = "modelica"` is an opt-in variant that no scene
//! in `assets/scenes/tests/` selects, so the parity scenes exercise the Rust
//! kernels and would stay green no matter what this move broke.

use lunco_modelica::ModelicaCompiler;

/// Compile by the short name and the `lunco://` path the USD cosim dispatcher
/// passes for `info:sourceAsset` — the same route a scene takes.
fn compile(model: &str) -> Box<rumoca_compile::compile::DaeCompilationResult> {
    let source = lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with(&format!("Mobility/{model}.mo")))
        .map(|(_, src)| src)
        .unwrap_or_else(|| panic!("{model} ships inside the LunCo package"));
    ModelicaCompiler::new()
        .compile_str(
            model,
            &source,
            &format!("lunco://models/LunCo/Mobility/{model}.mo"),
        )
        .unwrap_or_else(|e| panic!("{model} compiles as a package member: {e}"))
}

fn input_names(model: &str) -> Vec<String> {
    let dae = compile(model);
    let opts = rumoca_sim::SimOptions {
        t_start: 0.0,
        t_end: 10.0,
        ..Default::default()
    };
    let session = rumoca_sim::SimulationSession::new(&dae.dae, opts).expect("session builds");
    let mut names = session.input_names().to_vec();
    names.sort();
    names
}

fn outputs_of(model: &str) -> Vec<String> {
    let dae = compile(model);
    dae.dae
        .variables
        .outputs
        .iter()
        .chain(dae.dae.variables.algebraics.iter())
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The commands a driver actually sends have to arrive as runtime slots. An
/// empty list here is the silent-all-defaults failure, not a naming quibble.
#[test]
fn skid_law_still_accepts_its_driver_commands() {
    let names = input_names("RoverDrivetrain");
    assert!(
        !names.is_empty(),
        "ZERO inputs means every cosim wire into the skid drive law is rejected \
         and the rover drives on defaults — the exact failure the move risked"
    );
    for expected in ["throttle", "steer"] {
        assert!(
            names.iter().any(|n| n == expected),
            "`{expected}` must survive as a runtime slot; got {names:?}"
        );
    }
}

#[test]
fn ackermann_law_still_accepts_its_driver_commands() {
    let names = input_names("RoverAckermannDrivetrain");
    assert!(
        !names.is_empty(),
        "ZERO inputs means the Ackermann drive law runs on defaults"
    );
    assert!(
        names.iter().any(|n| n == "throttle"),
        "`throttle` must survive as a runtime slot; got {names:?}"
    );
}

/// The other half: what the wheels read back off the model. A law that accepts
/// commands but publishes nothing is just as dead.
#[test]
fn skid_law_still_publishes_per_side_demand() {
    let names = outputs_of("RoverDrivetrain");
    for expected in ["drive_left", "drive_right"] {
        assert!(
            names.iter().any(|n| n == expected || n.ends_with(expected)),
            "`{expected}` is what the USD wires fan onto the wheels; solved = {names:?}"
        );
    }
}
