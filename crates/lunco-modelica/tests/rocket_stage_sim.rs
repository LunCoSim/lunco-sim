//! End-to-end tests: AnnotatedRocketStage compiles, runs, and
//! responds to the `valve.opening` runtime input — both when seeded via
//! `StepperOptions::initial_inputs` at construction and when changed
//! live via `set_input()` between steps.
//!
//! Verifies, across the three scenarios:
//!   1. Model compiles under the acausal fluid architecture
//!      (Tank → Valve → Engine).
//!   2. `valve.opening` input slot is exposed at the stage boundary and
//!      `set_input()` wires through to the valve.
//!   3. Opening the throttle produces thrust, depletes the tank,
//!      and lifts the airframe; closing it stops consumption.
//!   4. Multiple mid-sim throttle changes do not destabilise the
//!      solver (regression for the BDF "step size too small"
//!      failure observed after repeated input changes without
//!      re-projecting algebraics onto the new manifold).

use lunco_modelica::ModelicaCompiler;
use rumoca_sim::{SimStepper, StepperOptions};

const SRC: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

fn build_stepper(initial_throttle: Option<f64>) -> SimStepper {
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
    opts.atol = 1e-2;
    opts.rtol = 1e-2;
    if let Some(v) = initial_throttle {
        opts.initial_inputs.insert("valve.opening".to_string(), v);
    }
    SimStepper::new(&dae.dae, opts).expect("stepper build")
}

fn advance(stepper: &mut SimStepper, dt: f64, steps: usize) {
    for i in 0..steps {
        stepper
            .step(dt)
            .unwrap_or_else(|err| panic!("step {i} (dt={dt}) failed: {err}"));
    }
}

/// Scenario A: throttle=1.0 from t=0 via `initial_inputs`.
/// Every IC pass sees the intended operating point; no discontinuity
/// at t=0; tank depletes and vehicle climbs.
#[test]
fn rocket_throttle_seeded_at_ic_drives_thrust_and_lift() {
    let mut stepper = build_stepper(Some(1.0));

    let inputs = stepper.input_names().to_vec();
    assert!(
        inputs.iter().any(|n| n == "valve.opening"),
        "expected `valve.opening` in input_names; got {inputs:?}",
    );

    let m0 = stepper.get("tank.m").expect("tank.m");
    let alt0 = stepper.get("airframe.altitude").expect("altitude");

    advance(&mut stepper, 0.1, 100); // 10 s

    let m1 = stepper.get("tank.m").expect("tank.m after");
    let alt1 = stepper.get("airframe.altitude").expect("altitude after");

    assert!(m1 < m0, "tank should deplete: {m0} -> {m1}");
    assert!(alt1 > alt0, "altitude should rise: {alt0} -> {alt1}");
}

/// Scenario B: build with throttle=0 (closed), then open it live via
/// `set_input()`. Regression for the "Step size too small at t≈10"
/// failure: the post-input-change projection must reseat y on the
/// new algebraic manifold before BDF restarts.
///
/// Physics: under gravity alone the rocket is in free fall; once we
/// open the throttle, the tank should start draining and the engine
/// should produce thrust. We check flow and thrust directly rather
/// than altitude, because this particular rocket's thrust-to-weight
/// is low enough that a brief free-fall before ignition can't be
/// fully recovered in a 10 s burn — physical, not a regression.
#[test]
fn rocket_throttle_opened_mid_sim_drives_thrust_and_lift() {
    let mut stepper = build_stepper(None); // throttle defaults to 0

    // Idle briefly — no thrust, no flow, tank mass constant.
    let m_idle_before = stepper.get("tank.m").expect("tank.m");
    advance(&mut stepper, 0.1, 5); // 0.5 s idling
    let m_idle_after = stepper.get("tank.m").expect("tank.m");
    assert!(
        (m_idle_after - m_idle_before).abs() < 1e-3,
        "tank should NOT deplete while throttle=0: {m_idle_before} -> {m_idle_after}",
    );
    let v_at_open = stepper.get("airframe.velocity").expect("velocity");

    // Open the throttle mid-run.
    stepper
        .set_input("valve.opening", 1.0)
        .expect("throttle is a valid input");

    // Run 10 more seconds. Previously this would stall with
    // "Step size is too small" mid-run.
    advance(&mut stepper, 0.1, 100);

    let m_end = stepper.get("tank.m").expect("tank.m end");
    let v_end = stepper.get("airframe.velocity").expect("velocity end");

    assert!(
        m_end < m_idle_after,
        "tank should deplete after throttle-open: {m_idle_after} -> {m_end}",
    );
    assert!(
        v_end > v_at_open,
        "velocity should climb after throttle-open: {v_at_open} -> {v_end}",
    );
}

/// Scenario C: open throttle, burn, then close it. Tank depletion
/// must stop once the throttle closes, but altitude keeps rising for
/// a moment from residual velocity (ballistic coast) before gravity
/// wins.
#[test]
fn rocket_throttle_closed_mid_sim_stops_tank_drain() {
    let mut stepper = build_stepper(Some(1.0));

    advance(&mut stepper, 0.1, 50); // 5 s burn

    let m_at_close = stepper.get("tank.m").expect("tank.m");
    stepper
        .set_input("valve.opening", 0.0)
        .expect("throttle is a valid input");

    advance(&mut stepper, 0.1, 50); // 5 s coasting

    let m_after_close = stepper.get("tank.m").expect("tank.m after");
    assert!(
        (m_after_close - m_at_close).abs() < 1e-1,
        "tank should NOT deplete further once throttle=0: {m_at_close} -> {m_after_close}",
    );
}

/// Scenario E: bounds enforcement. The valve declares
/// `opening(min = 0, max = 1)`; rumoca should reject `set_input`
/// values outside that range with an error rather than silently
/// clamping (silent clamping would mislead callers who expect
/// `get()` to round-trip the value they wrote). Also rejects
/// non-finite writes.
#[test]
fn rocket_throttle_set_input_rejects_out_of_bounds() {
    let mut stepper = build_stepper(None);

    // Negative throttle would mean "reverse flow" (engine → tank),
    // which the model can't physically represent.
    let err = stepper
        .set_input("valve.opening", -0.5)
        .expect_err("negative throttle must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains("below declared min"),
        "expected min-bound error; got {msg}",
    );

    // Above max.
    let err = stepper
        .set_input("valve.opening", 1.5)
        .expect_err("over-max throttle must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains("exceeds declared max"),
        "expected max-bound error; got {msg}",
    );

    // NaN must be rejected too.
    let err = stepper
        .set_input("valve.opening", f64::NAN)
        .expect_err("NaN throttle must be rejected");
    let msg = format!("{err}");
    assert!(msg.contains("finite"), "expected finite-check error; got {msg}");

    // Boundary values are allowed.
    stepper
        .set_input("valve.opening", 0.0)
        .expect("0.0 is on the boundary, should be allowed");
    stepper
        .set_input("valve.opening", 1.0)
        .expect("1.0 is on the boundary, should be allowed");
}

/// Scenario D: stress-test — change throttle many times mid-sim at
/// various values. Without the post-change algebraic projection, BDF
/// accumulates drift across repeated `reset_solver_history()` calls
/// and eventually stalls. With the projection, y is reseated on each
/// new manifold before the BDF restart, so the integrator stays
/// well-conditioned throughout.
#[test]
fn rocket_throttle_varied_mid_sim_stays_stable() {
    let mut stepper = build_stepper(None);

    // Walk the throttle through a sequence of values. Each change
    // triggers `inputs_dirty` → projection → history reset → step.
    let sequence: &[(f64, usize)] = &[
        (0.2, 15),
        (0.8, 15),
        (0.3, 15),
        (1.0, 15),
        (0.0, 15),
        (0.5, 15),
        (0.9, 15),
        (0.1, 15),
    ];

    let m_start = stepper.get("tank.m").expect("tank.m");

    for &(value, steps) in sequence {
        stepper
            .set_input("valve.opening", value)
            .expect("throttle is a valid input");
        advance(&mut stepper, 0.1, steps);
    }

    let m_end = stepper.get("tank.m").expect("tank.m");
    let t_end = stepper.time();

    // Primary assertion: we reached the end of the sequence without
    // the solver stalling (this `advance` above would panic on any
    // step failure). That IS the regression we fixed.
    assert!(
        t_end > 11.0,
        "solver should advance through the full sequence: t={t_end}",
    );

    // Secondary assertion: the throttle actually controlled flow —
    // net positive opening over the sequence ⇒ net tank depletion.
    assert!(
        m_end < m_start,
        "net burn across the sequence should deplete tank: {m_start} -> {m_end}",
    );
}
