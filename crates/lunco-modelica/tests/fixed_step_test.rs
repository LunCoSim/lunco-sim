use lunco_experiments::solver::{self, RuntimeProfile, SolverParams, SolverRequest};
use lunco_modelica::{
    fixed_step::FixedStepSession,
    simulation_session::{self, LiveStepper},
    solver_backends, ModelicaCompiler,
};
use rumoca_sim::{SimOptions, SimSolverMode};

fn ramp_model() -> &'static str {
    "model FixedRamp\n  Real x(start=0);\nequation\n  der(x) = 1;\nend FixedRamp;"
}

fn options() -> SimOptions {
    SimOptions {
        solver_mode: SimSolverMode::RkLike,
        dt: Some(0.01),
        t_start: 0.0,
        t_end: 1.0,
        ..Default::default()
    }
}

#[test]
fn fixed_rk4_uses_exactly_the_configured_step_lattice() {
    let mut compiler = ModelicaCompiler::new();
    let compiled = compiler
        .compile_str("FixedRamp", ramp_model(), "fixed_ramp.mo")
        .expect("fixed ramp compiles");
    let mut session = FixedStepSession::new(&compiled.dae, options()).expect("fixed session");

    for _ in 0..10 {
        session.step(0.01).expect("one fixed step");
    }

    assert!((session.time() - 0.1).abs() < 1.0e-14);
    let x = session.get("x").expect("read x").expect("x is visible");
    assert!((x - 0.1).abs() < 1.0e-12, "x={x}");
    assert!(session.step(0.02).is_err(), "variable dt must be rejected");
}

#[test]
fn fixed_rk4_repeats_the_same_operation_sequence() {
    let mut compiler = ModelicaCompiler::new();
    let compiled = compiler
        .compile_str("FixedRamp", ramp_model(), "fixed_ramp.mo")
        .expect("fixed ramp compiles");
    let mut left = FixedStepSession::new(&compiled.dae, options()).expect("left session");
    let mut right = FixedStepSession::new(&compiled.dae, options()).expect("right session");

    for _ in 0..100 {
        left.step(0.01).expect("left fixed step");
        right.step(0.01).expect("right fixed step");
    }

    assert_eq!(left.time().to_bits(), right.time().to_bits());
    assert_eq!(
        left.get("x")
            .expect("left x")
            .expect("left visible")
            .to_bits(),
        right
            .get("x")
            .expect("right x")
            .expect("right visible")
            .to_bits()
    );
}

#[test]
fn predicted_live_resolution_constructs_the_fixed_backend() {
    let mut compiler = ModelicaCompiler::new();
    let compiled = compiler
        .compile_str("FixedRamp", ramp_model(), "fixed_ramp.mo")
        .expect("fixed ramp compiles");

    solver_backends::ensure_builtin_solvers();
    let spec = solver::resolve(&SolverRequest {
        profile: RuntimeProfile {
            live: true,
            predicted: true,
        },
        authored: None,
    })
    .expect("prediction resolves to a deterministic backend");
    assert_eq!(spec.id, solver::SolverId::from("fixed-rk4"));

    let options = solver_backends::rumoca_options(
        &spec,
        &SolverParams {
            atol: 1.0e-6,
            rtol: 1.0e-6,
            h0: Some(0.01),
            t_start: 0.0,
            t_end: 1.0,
        },
    )
    .expect("the resolved backend maps to Rumoca options");
    let mut stepper = simulation_session::live(&compiled.dae, &spec, options)
        .expect("the resolved prediction backend constructs");

    assert!(matches!(&stepper, LiveStepper::Fixed(_)));
    stepper.step(0.01).expect("fixed prediction step");
    assert_eq!(stepper.time().to_bits(), 0.01_f64.to_bits());
}
