//! End-to-end test: AnnotatedRocketStage compiles and simulates.
//!
//! Exercises the real rocket physics (thrust, mass depletion, gravity)
//! by compiling `AnnotatedRocketStage.RocketStage` and stepping the
//! solver forward. Asserts altitude rises and tank mass falls —
//! catches regressions where the equations stop coupling (e.g. if
//! `tank.m_dot = engine.m_dot` ever gets elided).

use lunco_modelica::ModelicaCompiler;
use rumoca_sim::{SimStepper, StepperOptions};

const SRC: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

#[test]
fn rocket_stage_thrust_lifts_vehicle_and_depletes_tank() {
    let (stripped, _) = lunco_modelica::ast_extract::strip_input_defaults(SRC);

    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str("AnnotatedRocketStage.RocketStage", &stripped, "AnnotatedRocketStage.mo")
        .expect("RocketStage should compile");

    let mut opts = StepperOptions::default();
    opts.atol = 1e-1;
    opts.rtol = 1e-1;
    let mut stepper = SimStepper::new(&dae.dae, opts).expect("stepper build");

    let m0 = stepper.get("tank.m").expect("tank.m present");
    let alt0 = stepper.get("airframe.altitude").expect("altitude present");

    // Step 10 s of burn — long enough for thrust > weight to show.
    let mut t = 0.0;
    while t < 10.0 {
        stepper.step(t + 0.1).expect("step ok");
        t += 0.1;
    }

    let m1 = stepper.get("tank.m").expect("tank.m after");
    let alt1 = stepper.get("airframe.altitude").expect("altitude after");

    assert!(m1 < m0, "tank should deplete: {m0} -> {m1}");
    assert!(alt1 > alt0, "altitude should rise: {alt0} -> {alt1}");
}
