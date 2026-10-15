use lunco_experiments::solver::{self, RuntimeProfile, SolverParams, SolverRequest};
use lunco_modelica_solver::{
    fixed_step::FixedStepSession,
    simulation_session::{self, LiveStepper},
    solver_backends,
};
use rumoca_sim::{SimOptions, SimSolverMode};

fn compile(
    model: &str,
    source: &str,
    uri: &str,
) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
    let mut session = rumoca_compile::Session::default();
    session
        .add_document(uri, source)
        .map_err(|error| error.to_string())?;
    session.compile_model_dae_strict_reachable_uncached_with_recovery(model)
}

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
fn fixed_rk4_reads_inputs_and_algebraic_values_from_checked_runtime() {
    let compiled = compile("Coupled", "model Coupled input Real u = 2; Real x(start=0); output Real y; equation y=3*u+x; der(x)=y; end Coupled;", "coupled.mo").unwrap();
    let mut session = FixedStepSession::new(&compiled.dae, options()).unwrap();
    assert_eq!(session.state().unwrap().values.get("u").copied(), Some(2.0));
    assert_eq!(session.state().unwrap().values.get("y").copied(), Some(6.0));
    assert!(session.set_inputs(&[("u", f64::NAN)]).is_err());
    assert!(session.set_inputs(&[("u", 9.0), ("missing", 1.0)]).is_err());
    assert_eq!(session.state().unwrap().values.get("u").copied(), Some(2.0));
    session.step(0.01).unwrap();
    let x = session.state().unwrap().values.get("x").copied().unwrap();
    assert!((x - 6.0 * (0.01_f64.exp() - 1.0)).abs() < 1.0e-10);
    assert!(
        (session.state().unwrap().values.get("y").copied().unwrap() - (6.0 + x)).abs() < 1.0e-10
    );
}

#[test]
fn fixed_rk4_uses_exactly_the_configured_step_lattice() {
    let compiled =
        compile("FixedRamp", ramp_model(), "fixed_ramp.mo").expect("fixed ramp compiles");
    let mut session = FixedStepSession::new(&compiled.dae, options()).expect("fixed session");

    for _ in 0..10 {
        session.step(0.01).expect("one fixed step");
    }

    assert!((session.time() - 0.1).abs() < 1.0e-14);
    let x = session.state().expect("read state").values["x"];
    assert!((x - 0.1).abs() < 1.0e-12, "x={x}");
    assert!(session.step(0.02).is_err(), "variable dt must be rejected");
    for invalid_dt in [0.0, -0.01, f64::NAN, f64::INFINITY] {
        assert!(session.step(invalid_dt).is_err());
    }
    assert!((session.time() - 0.1).abs() < 1.0e-14);
}

#[test]
fn fixed_communication_points_preserve_internal_steps_and_reject_invalid_targets() {
    let compiled = compile("FixedRamp", ramp_model(), "fixed_ramp.mo").unwrap();
    for origin in [0.0, 2.75] {
        let mut settings = options();
        settings.t_start = origin;
        let mut interval = FixedStepSession::new(&compiled.dae, settings.clone()).unwrap();
        let mut single = FixedStepSession::new(&compiled.dae, settings).unwrap();
        interval.advance_to(origin + 0.06).unwrap();
        for _ in 0..6 {
            single.step(0.01).unwrap();
        }
        let expected = single.state().unwrap();
        assert_eq!(
            interval.state().unwrap().time.to_bits(),
            expected.time.to_bits()
        );
        assert_eq!(interval.state().unwrap().values, expected.values);
        interval.advance_to(origin + 0.06).unwrap();
        for target in [
            origin + 0.065,
            origin + 0.05,
            f64::NAN,
            f64::INFINITY,
            1e300,
        ] {
            assert!(
                interval.advance_to(target).is_err(),
                "invalid target {target}"
            );
            assert_eq!(
                interval.state().unwrap().time.to_bits(),
                expected.time.to_bits()
            );
            assert_eq!(interval.state().unwrap().values, expected.values);
        }
    }
}

#[test]
fn fixed_rk4_repeats_the_same_operation_sequence() {
    let compiled =
        compile("FixedRamp", ramp_model(), "fixed_ramp.mo").expect("fixed ramp compiles");
    let mut left = FixedStepSession::new(&compiled.dae, options()).expect("left session");
    let mut right = FixedStepSession::new(&compiled.dae, options()).expect("right session");

    for _ in 0..100 {
        left.step(0.01).expect("left fixed step");
        right.step(0.01).expect("right fixed step");
    }

    assert_eq!(left.time().to_bits(), right.time().to_bits());
    assert_eq!(
        left.state().expect("left state").values["x"].to_bits(),
        right.state().expect("right state").values["x"].to_bits()
    );
}

#[test]
fn predicted_live_resolution_constructs_the_fixed_backend() {
    let compiled =
        compile("FixedRamp", ramp_model(), "fixed_ramp.mo").expect("fixed ramp compiles");

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
    stepper.advance_to(0.01).expect("fixed prediction step");
    assert_eq!(stepper.time().to_bits(), 0.01_f64.to_bits());
}

/// Low-level deterministic integration and live backend selection.
///
/// This deliberately drives two independently constructed `LiveStepper`s
/// through the same changing input schedule and compares every published
/// visible value bit-for-bit after every exact fixed step. A unit test of
/// `FixedStepSession` alone would not catch the resolver or construction path
/// accidentally selecting an adaptive backend.
#[test]
fn predicted_live_runtime_is_bitwise_deterministic_over_an_input_schedule() {
    let compiled = compile("FixedInputRamp", input_ramp_model(), "fixed_input_ramp.mo")
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
    assert_eq!(left.state().unwrap().values.get("u").copied(), Some(0.0));

    for step in 0..512 {
        let input = ((step % 11) as f64 - 5.0) * 0.125;
        left.set_inputs(&[("u", input)])
            .expect("left input is accepted");
        right
            .set_inputs(&[("u", input)])
            .expect("right input is accepted");
        assert_eq!(left.state().unwrap().values.get("u").copied(), Some(input));
        left.advance_to((step + 1) as f64 * 0.01)
            .expect("left communication point");
        right
            .advance_to((step + 1) as f64 * 0.01)
            .expect("right communication point");

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
