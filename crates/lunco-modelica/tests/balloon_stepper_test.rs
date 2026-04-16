//! End-to-end test that exercises rumoca directly with balloon.mo.
//!
//! This bypasses the Bevy ECS entirely and asks two questions:
//! 1. After compiling balloon.mo and creating a `SimStepper`, what does
//!    `stepper.variable_names()` return?
//! 2. Can `stepper.get("netForce")` retrieve the algebraic value by name?
//!
//! If `get("netForce")` returns `None`, rumoca has eliminated the algebraic
//! from its solver index entirely — and `collect_all_variables` can't recover
//! it without upstream changes. If it returns `Some`, the problem is upstream
//! of the stepper (something else in lunco-modelica isn't calling `get`).

use lunco_modelica::{ModelicaCompiler, extract_variable_names};
use rumoca_sim::{SimStepper, StepperOptions};

const BALLOON_MO: &str = include_str!("../../../assets/models/balloon.mo");

#[test]
fn balloon_stepper_variable_names_contain_states_only() {
    // Strip input defaults so `input Real height = 0` becomes a runtime slot.
    let (stripped, _defaults) = lunco_modelica::ast_extract::strip_input_defaults(BALLOON_MO);

    let mut compiler = ModelicaCompiler::new();
    let dae_result = compiler
        .compile_str("Balloon", &stripped, "balloon.mo")
        .expect("balloon.mo should compile cleanly");

    let mut opts = StepperOptions::default();
    opts.atol = 1e-3;
    opts.rtol = 1e-3;
    let stepper = SimStepper::new(&dae_result.dae, opts).expect("stepper build");

    let solver_names: Vec<String> = stepper.variable_names().to_vec();
    eprintln!("solver variable_names = {:?}", solver_names);

    // Print what the AST walker sees as continuous non-input/non-parameter vars.
    let ast_names = extract_variable_names(BALLOON_MO);
    eprintln!("ast continuous vars = {:?}", ast_names);

    // Sanity: the state `volume` should always be present.
    assert!(
        solver_names.contains(&"volume".to_string()),
        "solver should list volume; got {:?}",
        solver_names
    );
}

#[test]
fn balloon_stepper_get_recovers_algebraics() {
    let (stripped, _defaults) = lunco_modelica::ast_extract::strip_input_defaults(BALLOON_MO);

    let mut compiler = ModelicaCompiler::new();
    let dae_result = compiler
        .compile_str("Balloon", &stripped, "balloon.mo")
        .expect("balloon.mo should compile cleanly");

    let mut opts = StepperOptions::default();
    opts.atol = 1e-3;
    opts.rtol = 1e-3;
    let stepper = SimStepper::new(&dae_result.dae, opts).expect("stepper build");

    // Names we expect to be recoverable by stepper.get() (either by being in
    // the solver index or by rumoca exposing them somehow).
    let probes = [
        "volume",
        "temperature",
        "airDensity",
        "buoyancy",
        "weight",
        "drag",
        "netForce",
    ];
    for name in probes {
        let val = stepper.get(name);
        eprintln!("stepper.get({}) = {:?}", name, val);
    }

    // Hard requirement: we must be able to get netForce by name.
    // If this assert fails, rumoca has eliminated netForce from the solver
    // entirely, and the workaround in collect_all_variables cannot recover it.
    // The fix would then have to move upstream (into the rumoca fork) — either
    // by not eliminating user-facing algebraics, or by exposing a separate
    // "residual evaluation" API.
    assert!(
        stepper.get("netForce").is_some(),
        "rumoca stepper should allow get(\"netForce\"), got None — \
         algebraic substitution has eliminated it from the solver index"
    );
}

#[test]
fn balloon_stepper_initial_netforce_is_positive() {
    // If this passes, netForce > 0 at the initial condition (balloon wants to rise).
    let (stripped, _defaults) = lunco_modelica::ast_extract::strip_input_defaults(BALLOON_MO);

    let mut compiler = ModelicaCompiler::new();
    let dae_result = compiler
        .compile_str("Balloon", &stripped, "balloon.mo")
        .expect("balloon.mo should compile cleanly");

    let mut opts = StepperOptions::default();
    opts.atol = 1e-3;
    opts.rtol = 1e-3;
    let mut stepper = SimStepper::new(&dae_result.dae, opts).expect("stepper build");

    // Step once so algebraics get evaluated if they're only computed on step().
    let _ = stepper.step(0.016);

    let net_force = stepper.get("netForce");
    eprintln!("netForce after first step = {:?}", net_force);
    let volume = stepper.get("volume");
    eprintln!("volume after first step = {:?}", volume);

    // buoyancy = rho * V * g ≈ 1.225 * 4.0 * 9.81 ≈ 48 N
    // weight = m * g = 4.5 * 9.81 ≈ 44 N
    // netForce ≈ 4 N > 0
    assert!(
        net_force.map(|v| v > 0.0).unwrap_or(false),
        "balloon should have positive netForce at t=0, got {:?}",
        net_force
    );
}
