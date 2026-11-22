use lunco_modelica_solver::simulation_session;
use rumoca_sim::{SimOptions, SimSolverMode};

fn compiled_ramp() -> Box<rumoca_compile::compile::DaeCompilationResult> {
    let source = "model Ramp\n  input Real u = 1;\n  Real x(start=0, fixed=true);\n  output Real y;\nequation\n  der(x) = u;\n  y = 2*x;\nend Ramp;";
    let mut compiler = rumoca_compile::Session::default();
    compiler.add_document("ramp.mo", source).unwrap();
    compiler
        .compile_model_dae_strict_reachable_uncached_with_recovery("Ramp")
        .unwrap()
}

#[test]
fn luncosim_live_adapter_uses_rumoca_session_for_inputs_steps_and_observations() {
    let compiled = compiled_ramp();
    let options = SimOptions {
        solver_mode: SimSolverMode::RkLike,
        dt: Some(0.01),
        t_start: 0.0,
        t_end: 1.0,
        ..Default::default()
    };
    let mut live = simulation_session::live(&compiled.dae, options)
        .expect("Rumoca should construct the admitted live session");

    assert_eq!(live.time(), 0.0);
    assert_eq!(live.input_names(), &["u".to_owned()]);
    live.set_inputs(&[("u", 2.0)])
        .expect("the current Rumoca session accepts an input transaction");
    live.advance_to(0.5)
        .expect("the current Rumoca session reaches the requested communication point");

    assert_eq!(live.time(), 0.5);
    let values = live
        .selected_values(&["x".to_owned(), "y".to_owned(), "u".to_owned()])
        .expect("the session exposes the selected observations");
    assert_eq!(values.len(), 3);
    assert!((values[0].expect("state value is present") - 1.0).abs() < 1.0e-6);
    assert!((values[1].expect("output value is present") - 2.0).abs() < 1.0e-6);
    assert_eq!(values[2], Some(2.0));
}

#[test]
fn fixed_rk4_adapter_uses_rumocas_model_exchange_integrator() {
    let compiled = compiled_ramp();
    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();
    let spec = lunco_experiments::solver::resolve(&lunco_experiments::solver::SolverRequest {
        profile: lunco_experiments::solver::RuntimeProfile {
            live: true,
            predicted: true,
        },
        authored: None,
    })
    .expect("prediction resolves to the fixed-step capability");
    let options = lunco_modelica_solver::solver_backends::rumoca_options(
        &spec,
        &lunco_experiments::solver::SolverParams {
            atol: 1.0e-6,
            rtol: 1.0e-6,
            h0: Some(0.01),
            t_start: 0.0,
            t_end: 1.0,
        },
    )
    .expect("Rumoca receives the Model Exchange execution family");
    let mut live = simulation_session::live_for_solver(&compiled.dae, &spec, options)
        .expect("the adapter installs fixed RK4 for predicted sessions");

    live.set_inputs(&[("u", 2.0)])
        .expect("the ME host accepts the complete input transaction");
    live.advance_to(0.05)
        .expect("the adapter advances on its 10 ms fixed lattice");

    assert_eq!(live.time(), 0.05);
    let values = live
        .selected_values(&["y".to_owned(), "u".to_owned()])
        .expect("fixed-step observations use Rumoca's visible values");
    assert!((values[0].expect("output y is present") - 0.2).abs() < 1.0e-6);
    assert_eq!(values[1], Some(2.0));
}

#[test]
fn string_metadata_parameters_are_not_projected_as_numeric_values() {
    let source = r#"model StringMetadata
        parameter String label="bulk mixer";
        parameter Real gain=2;
        output Real y;
    equation
        y=gain;
    end StringMetadata;"#;
    let mut compiler = rumoca_compile::Session::default();
    compiler.add_document("StringMetadata.mo", source).unwrap();
    let compiled = compiler
        .compile_model_dae_strict_reachable_uncached_with_recovery("StringMetadata")
        .unwrap();
    let mut live = simulation_session::live(&compiled.dae, SimOptions::default())
        .expect("nonnumeric metadata must not enter the numeric parameter projection");

    assert!(
        live.parameter_values()
            .iter()
            .any(|(name, value)| { name == "gain" && (*value - 2.0).abs() < f64::EPSILON })
    );
    assert!(
        live.parameter_values()
            .iter()
            .all(|(name, _)| name != "label"),
        "String metadata has no f64 solver value"
    );
    live.advance_to(0.1)
        .expect("string metadata does not prevent numerical stepping");
    assert_eq!(live.get("y").unwrap(), Some(2.0));

    lunco_modelica_solver::solver_backends::ensure_builtin_solvers();
    let spec = lunco_experiments::solver::resolve(&lunco_experiments::solver::SolverRequest {
        profile: lunco_experiments::solver::RuntimeProfile {
            live: true,
            predicted: true,
        },
        authored: None,
    })
    .expect("prediction resolves to the fixed-step capability");
    let options = lunco_modelica_solver::solver_backends::rumoca_options(
        &spec,
        &lunco_experiments::solver::SolverParams {
            atol: 1.0e-6,
            rtol: 1.0e-6,
            h0: Some(0.01),
            t_start: 0.0,
            t_end: 1.0,
        },
    )
    .expect("Rumoca receives the Model Exchange execution family");
    let mut fixed = simulation_session::live_for_solver(&compiled.dae, &spec, options)
        .expect("fixed-step sessions must also skip nonnumeric metadata");
    fixed
        .advance_to(0.05)
        .expect("fixed-step sessions advance with String metadata present");
    assert_eq!(fixed.get("y").unwrap(), Some(2.0));
}
