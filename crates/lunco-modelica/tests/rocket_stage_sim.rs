//! End-to-end test: AnnotatedRocketStage compiles and simulates.
//!
//! Throttle is a parameter (not a runtime input) because rumoca's
//! BDF initial-condition solver stalls on composite trees with
//! runtime inputs today — see the note on `RocketStage.throttle` in
//! the .mo file. The workbench dispatches UpdateParameters on
//! Telemetry edits, which recompiles quickly enough to feel live.

use lunco_modelica::ModelicaCompiler;
use rumoca_sim::{SimStepper, StepperOptions};

const SRC: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

#[test]
fn rocket_stage_thrust_lifts_vehicle_and_depletes_tank() {
    let (stripped, _) = lunco_modelica::ast_extract::strip_input_defaults(SRC);

    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "AnnotatedRocketStage.RocketStage",
            &stripped,
            "AnnotatedRocketStage.mo",
        )
        .expect("RocketStage should compile");

    let mut opts = StepperOptions::default();
    opts.atol = 1e-1;
    opts.rtol = 1e-1;
    let mut stepper = SimStepper::new(&dae.dae, opts).expect("stepper build");

    let m0 = stepper.get("tank.m").expect("tank.m present");
    let alt0 = stepper.get("airframe.altitude").expect("altitude present");

    let mut t = 0.0;
    while t < 10.0 {
        stepper.step(0.1).expect("step ok");
        t += 0.1;
    }

    let m1 = stepper.get("tank.m").expect("tank.m after");
    let alt1 = stepper.get("airframe.altitude").expect("altitude after");

    assert!(m1 < m0, "tank should deplete: {m0} -> {m1}");
    assert!(alt1 > alt0, "altitude should rise: {alt0} -> {alt1}");
}
