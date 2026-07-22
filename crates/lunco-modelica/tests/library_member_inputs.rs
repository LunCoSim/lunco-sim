//! A library class's bound `input`s must survive compilation as runtime slots.
//!
//! `compile_str` routes a `.mo` that declares `within P;` down the LIBRARY path:
//! the file is never seated as a document, the whole package is, and the
//! qualified class is compiled out of it (see `compile_str`'s comment on why —
//! seating it as well produces a duplicate-class rejection).
//!
//! That path used to hand the package to rumoca's own source-root loader, which
//! reads the files off disk itself. `strip_input_defaults` therefore never ran on
//! a library member, rumoca demoted every `input Real x = <default>` to an
//! algebraic, and `SimulationSession::input_names()` came back EMPTY. The cosim
//! then rejected each wire into the model and it held its declared defaults for
//! the whole run — a silent all-defaults simulation that renders as plausible,
//! wrong footage (episode 01's descent burn lit no plume: `PlumePhotometry`
//! never received `throttle`).

fn input_names_of(model: &str) -> Vec<String> {
    let mut compiler = lunco_modelica::ModelicaCompiler::new();
    let source = std::fs::read_to_string(model_path(model)).expect("library member readable");
    let dae = compiler
        .compile_str(model, &source, &model_path(model))
        .unwrap_or_else(|e| panic!("{model} compiles: {e}"));
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

fn model_path(model: &str) -> String {
    let leaf = model.rsplit('.').next().unwrap_or(model);
    format!(
        "{}/../../assets/models/LunCo/Propulsion/{leaf}.mo",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn library_member_bound_input_survives_as_runtime_slot() {
    let names = input_names_of("PlumePhotometry");
    assert!(
        names.iter().any(|n| n == "throttle"),
        "`input Real throttle = 0.0` in a library member must stay a runtime slot; \
         got {names:?} — the bound-input strip was skipped on the library path, so \
         every wire into this model is rejected and it runs on its defaults"
    );
    for expected in [
        "throttle",
        "w_max",
        "l_max",
        "width_idle",
        "luminance",
        "exitance",
        "r_idle",
        "r_gain",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "PlumePhotometry should expose `{expected}` as a runtime input; got {names:?}"
        );
    }
}

#[test]
fn library_member_exposes_every_bound_input() {
    let names = input_names_of("BellNozzle");
    for expected in [
        "p_chamber",
        "p_exit",
        "p_ambient",
        "gamma",
        "throat_radius",
        "exit_radius",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "BellNozzle should expose `{expected}` as a runtime input; got {names:?}"
        );
    }
}
