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

fn input_ramp_model() -> &'static str {
    "model FixedInputRamp\n  input Real u = 0;\n  Real x(start=0);\nequation\n  der(x) = u;\nend FixedInputRamp;"
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

/// Production acceptance for the predicted/live construction boundary.
///
/// This deliberately drives two independently constructed `LiveStepper`s
/// through the same changing input schedule and compares every published
/// visible value bit-for-bit after every exact fixed step. A unit test of
/// `FixedStepSession` alone would not catch the resolver or construction path
/// accidentally selecting an adaptive backend.
#[test]
fn predicted_live_runtime_is_bitwise_deterministic_over_an_input_schedule() {
    let mut compiler = ModelicaCompiler::new();
    let compiled = compiler
        .compile_str("FixedInputRamp", input_ramp_model(), "fixed_input_ramp.mo")
        .expect("fixed input ramp compiles");

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
            t_end: 100.0,
        },
    )
    .expect("fixed-rk4 options are valid");
    let mut left = simulation_session::live(&compiled.dae, &spec, options.clone())
        .expect("left live stepper constructs");
    let mut right = simulation_session::live(&compiled.dae, &spec, options)
        .expect("right live stepper constructs");

    assert!(matches!(&left, LiveStepper::Fixed(_)));
    assert!(matches!(&right, LiveStepper::Fixed(_)));
    assert_eq!(left.input_names(), &["u".to_string()]);

    for step in 0..512 {
        let input = ((step % 11) as f64 - 5.0) * 0.125;
        left.set_input("u", input).expect("left input is accepted");
        right
            .set_input("u", input)
            .expect("right input is accepted");
        left.step(0.01).expect("left fixed step");
        right.step(0.01).expect("right fixed step");

        let left_state = left.state().expect("left state is observable");
        let right_state = right.state().expect("right state is observable");
        assert_eq!(
            left_state.time.to_bits(),
            right_state.time.to_bits(),
            "step {step}"
        );
        assert_eq!(
            left_state.values.len(),
            right_state.values.len(),
            "step {step}"
        );
        for ((left_name, left_value), (right_name, right_value)) in
            left_state.values.iter().zip(right_state.values.iter())
        {
            assert_eq!(left_name, right_name, "step {step}");
            assert_eq!(
                left_value.to_bits(),
                right_value.to_bits(),
                "step {step}, {left_name}"
            );
        }
    }
}
