//! Bisection of a rumoca structural-analysis bug that kills
//! simulation of many MSL examples (PID_Controller,
//! OvervoltageProtection, ResonanceCircuits, TestSensors, …).
//! All affected models stall on the first step with
//! `SolverError("Step size is too small")` or
//! `Exceeded maximum number of nonlinear solver failures`.
//!
//! ## Symptom in the sim trace
//!
//! ```text
//! IC plan singular detail: unmatched_eq=1 unmatched_unknowns=2
//!   unmatched_unknown 'spring.w_rel'  referenced_by=2
//!   unmatched_unknown 'ptp.Tv'        referenced_by=1
//!   unmatched_eq f_x[6] origin='equation from spring' unknowns=
//! ```
//!
//! The key clue is `unknowns=` (empty) — an equation in the
//! post-`eliminate_trivial` DAE has been reduced to a tautology
//! (`0 = 0`) by the substitution chain. It still occupies a
//! matching slot but has nothing to match, so real unknowns like
//! `spring.w_rel` and `ptp.Tv` end up structurally homeless. The
//! downstream runtime projection then diverges (residual grows
//! from 670 → 9 billion over 5 iterations) because the Jacobian
//! is genuinely rank-deficient.
//!
//! ## Minimum trigger (confirmed by the bisection below)
//!
//! All three conditions required:
//!   1. A `KinematicPTP`-style block — conditional equations over
//!      a discrete Boolean (`if noWphase then … else …`) are what
//!      seem to confuse the matcher.
//!   2. A sensor connected into a connect-set with ≥3 elements
//!      (e.g. `speedSensor.flange` wired to an `inertia.flange_b`
//!      that also has `spring.flange_a` — flow-conservation row
//!      `tau_a + tau_b + tau_c = 0`).
//!   3. A **purely algebraic** feedback path from sensor to
//!      actuator through the KinematicPTP-derived setpoint
//!      (plain `Gain`, or `LimPID`'s anti-windup loop). When
//!      the feedback passes through a block with its own
//!      continuous state (e.g. `Modelica.Blocks.Continuous.PID`),
//!      the algebraic chain breaks and the bug doesn't trigger.
//!
//! ## Fix location (hypothesis)
//!
//! `rumoca-phase-solve/src/eliminate/mod.rs` — specifically
//! `resolve_boundary_equations`'s substitution cascade. A
//! substitution turns a conservation row into a tautology, and
//! nothing backs it out. The targeted fix: after applying
//! `X → expr`, scan the remaining equations; if any become
//! structurally empty AND their origin is `"flow sum equation"`,
//! back out the substitution and keep `X` as an unknown.
//!
//! Run with:
//!   RUMOCA_SIM_TRACE=1 cargo test --package lunco-modelica \
//!       --test rumoca_structural_bug_bisection -- --nocapture
//!
//! Tests marked `#[ignore]` are the ones that currently fail due
//! to this bug — `cargo test` skips them by default so CI stays
//! green; run with `cargo test -- --ignored` to reproduce.

use rumoca_sim::{SimStepper, StepperOptions};

fn try_run(label: &str, model_name: &str, source: &str) -> Result<(), String> {
    let mut compiler = lunco_modelica::ModelicaCompiler::new();
    let dae = compiler
        .compile_str(model_name, source, "tier.mo")
        .map_err(|e| format!("{label}: compile failed: {e}"))?;
    let mut opts = StepperOptions::default();
    opts.atol = 1e-2;
    opts.rtol = 1e-2;
    let mut stepper = SimStepper::new(&dae.dae, opts)
        .map_err(|e| format!("{label}: stepper build failed: {e}"))?;
    stepper
        .step(0.001)
        .map_err(|e| format!("{label}: step failed: {e}"))?;
    let t = stepper.time();
    println!("✓ {label}: stepped to t = {t}");
    Ok(())
}

/// T0 — bare Inertia driven by a ConstantTorque load.
const T0_SRC: &str = r#"
model T0
  Modelica.Mechanics.Rotational.Components.Inertia inertia(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
equation
  connect(load.flange, inertia.flange_a);
end T0;
"#;

#[test]
fn tier_0_bare_inertia_with_constant_torque() {
    try_run("T0", "T0", T0_SRC).unwrap();
}

/// T1 — two inertias coupled by SpringDamper.
const T1_SRC: &str = r#"
model T1
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
equation
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
end T1;
"#;

#[test]
fn tier_1_two_inertias_with_spring_damper() {
    try_run("T1", "T1", T1_SRC).unwrap();
}

/// T2 — add a Torque source on inertia1.
const T2_SRC: &str = r#"
model T2
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.Constant tauCmd(k=2);
equation
  connect(tauCmd.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
end T2;
"#;

#[test]
fn tier_2_add_torque_source() {
    try_run("T2", "T2", T2_SRC).unwrap();
}

/// T3 — add a SpeedSensor on inertia1.
const T3_SRC: &str = r#"
model T3
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.Constant tauCmd(k=2);
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Real measured;
equation
  measured = speedSensor.w;
  connect(tauCmd.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T3;
"#;

#[test]
fn tier_3_add_speed_sensor() {
    try_run("T3", "T3", T3_SRC).unwrap();
}

/// T4 — add KinematicPTP source feeding an Integrator (no controller yet).
const T4_SRC: &str = r#"
model T4
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
equation
  connect(ptp.y[1], integrator.u);
  connect(integrator.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
end T4;
"#;

#[test]
fn tier_4_add_kinematic_ptp_source() {
    try_run("T4", "T4", T4_SRC).unwrap();
}

/// T4a — T4 + a SpeedSensor on inertia1 (does not feed anything).
const T4A_SRC: &str = r#"
model T4a
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
equation
  connect(ptp.y[1], integrator.u);
  connect(integrator.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4a;
"#;

#[test]
fn tier_4a_add_speed_sensor_unused() {
    try_run("T4a", "T4a", T4A_SRC).unwrap();
}

/// T4b — T4 + LimPID with default (no SteadyState) init.
/// PI mode, default tuning, default initType. Replaces integrator→torque
/// with integrator→PI.u_s, sensor→PI.u_m, PI.y→torque.tau.
const T4B_SRC: &str = r#"
model T4b
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Continuous.LimPID PI(
    k=100, Ti=0.1, Td=0.1, yMax=12, Ni=0.1,
    controllerType=Modelica.Blocks.Types.SimpleController.PI);
equation
  connect(ptp.y[1], integrator.u);
  connect(integrator.y, PI.u_s);
  connect(speedSensor.w, PI.u_m);
  connect(PI.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4b;
"#;

#[test]
#[ignore = "rumoca structural bug: LimPID + closed-loop feedback trips eliminate_trivial"]
fn tier_4b_add_lim_pid_default_init() {
    try_run("T4b", "T4b", T4B_SRC).unwrap();
}

/// T4c — T4b + initType=SteadyState on the PID. This is what the
/// MSL PID_Controller example uses. Suspected key trigger.
const T4C_SRC: &str = r#"
model T4c
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Continuous.LimPID PI(
    k=100, Ti=0.1, Td=0.1, yMax=12, Ni=0.1,
    initType=Modelica.Blocks.Types.Init.SteadyState,
    controllerType=Modelica.Blocks.Types.SimpleController.PI);
equation
  connect(ptp.y[1], integrator.u);
  connect(integrator.y, PI.u_s);
  connect(speedSensor.w, PI.u_m);
  connect(PI.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4c;
"#;

#[test]
#[ignore = "rumoca structural bug"]
fn tier_4c_add_lim_pid_steadystate_init() {
    try_run("T4c", "T4c", T4C_SRC).unwrap();
}

/// T4b' — like T4b but with `Modelica.Blocks.Continuous.PID`
/// (no anti-windup limiter) instead of LimPID. Rumoca's own
/// PIDMSL example uses this block and runs successfully, so if
/// this also passes the bug is specific to LimPID's saturation
/// machinery (Limiter sub-block + addSat anti-windup feedback).
const T4B_PRIME_SRC: &str = r#"
model T4b_prime
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Continuous.PID PI(k=100, Ti=0.1, Td=0.1);
  Real err;
equation
  err = integrator.y - speedSensor.w;
  PI.u = err;
  connect(ptp.y[1], integrator.u);
  connect(PI.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4b_prime;
"#;

#[test]
fn tier_4b_prime_plain_pid_no_saturation() {
    try_run("T4b'", "T4b_prime", T4B_PRIME_SRC).unwrap();
}

/// T4d — replace LimPID with a plain Gain block. Confirms whether
/// it's specifically a CONTINUOUS block (with internal states) or
/// the closed-loop topology itself that breaks rumoca.
const T4D_SRC: &str = r#"
model T4d
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
  Real err;
equation
  err = integrator.y - speedSensor.w;
  gain.u = err;
  connect(ptp.y[1], integrator.u);
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4d;
"#;

#[test]
#[ignore = "rumoca structural bug: KinematicPTP + sensor-in-connect-set + algebraic feedback"]
fn tier_4d_closed_loop_with_gain_only() {
    try_run("T4d", "T4d", T4D_SRC).unwrap();
}

/// T4d-min — strip away every part of T4d not directly needed to
/// reproduce the failure. Goal: smallest possible model that
/// triggers the over-eager substitution.
const T4D_MIN_SRC: &str = r#"
model T4d_min
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
equation
  // Closed loop: torque tau = -gain * w (negative feedback).
  gain.u = speedSensor.w;
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(speedSensor.flange, inertia1.flange_b);
end T4d_min;
"#;

#[test]
fn tier_4d_min_minimal_failing_loop() {
    try_run("T4d-min", "T4d_min", T4D_MIN_SRC).unwrap();
}

/// T4d-min-2 — same as T4d-min but speedSensor connects to a
/// SEPARATE inertia flange (not the same connect-set as the
/// spring). Tests whether the bug is specifically about a
/// 3-element connect-set with the sensor.
const T4D_MIN_2_SRC: &str = r#"
model T4d_min_2
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
equation
  gain.u = speedSensor.w;
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  // Sensor on the OTHER side of the spring.
  connect(speedSensor.flange, inertia2.flange_b);
end T4d_min_2;
"#;

#[test]
fn tier_4d_min_2_sensor_on_other_side() {
    try_run("T4d-min-2", "T4d_min_2", T4D_MIN_2_SRC).unwrap();
}

/// T4d-noptp — T4d but with `Constant` source (no KinematicPTP).
/// Validates that KinematicPTP is the necessary co-trigger.
const T4D_NOPTP_SRC: &str = r#"
model T4d_noptp
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.Constant setpoint(k=1);
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
  Real err;
equation
  err = integrator.y - speedSensor.w;
  gain.u = err;
  connect(setpoint.y, integrator.u);
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4d_noptp;
"#;

#[test]
fn tier_4d_noptp_constant_source() {
    try_run("T4d-noptp", "T4d_noptp", T4D_NOPTP_SRC).unwrap();
}

/// T4d-onlyptp — minimal failing case = T4d-min + KinematicPTP
/// (no integrator, no setpoint complication). Confirms KinematicPTP
/// alone (just instantiated, not even connected) suffices.
const T4D_ONLYPTP_SRC: &str = r#"
model T4d_onlyptp
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
  // Just sitting there, dangling — does NOT feed into the loop.
  Modelica.Blocks.Sources.KinematicPTP ptp(deltaq={1}, qd_max={1}, qdd_max={1});
equation
  gain.u = speedSensor.w;
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(speedSensor.flange, inertia1.flange_b);
end T4d_onlyptp;
"#;

#[test]
fn tier_4d_onlyptp_dangling_kinematic_ptp() {
    try_run("T4d-onlyptp", "T4d_onlyptp", T4D_ONLYPTP_SRC).unwrap();
}

/// T4d-nosensor — T4d with KinematicPTP + Gain, but NO speed
/// sensor feedback (loop is open). Should pass — equivalent to T4.
const T4D_NOSENSOR_SRC: &str = r#"
model T4d_nosensor
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Blocks.Continuous.Integrator integrator;
  Modelica.Blocks.Math.Gain gain(k=10);
equation
  gain.u = integrator.y;
  connect(ptp.y[1], integrator.u);
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
end T4d_nosensor;
"#;

#[test]
fn tier_4d_nosensor_ptp_integrator_no_loop() {
    try_run("T4d-nosensor", "T4d_nosensor", T4D_NOSENSOR_SRC).unwrap();
}

/// T4d-ptpdirect — closed loop with sensor, KinematicPTP as
/// setpoint (direct, no integrator between). Tests whether the
/// integrator matters or just KinematicPTP.
const T4D_PTPDIRECT_SRC: &str = r#"
model T4d_ptpdirect
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J=1, phi(fixed=true, start=0), w(fixed=true, start=0));
  Modelica.Mechanics.Rotational.Components.SpringDamper spring(c=1e4, d=100);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(J=2);
  Modelica.Mechanics.Rotational.Sources.ConstantTorque load(tau_constant=1, useSupport=false);
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport=false);
  Modelica.Blocks.Sources.KinematicPTP ptp(startTime=0.5, deltaq={1}, qd_max={1}, qdd_max={1});
  Modelica.Mechanics.Rotational.Sensors.SpeedSensor speedSensor;
  Modelica.Blocks.Math.Gain gain(k=10);
  Real err;
equation
  err = ptp.y[1] - speedSensor.w;
  gain.u = err;
  connect(gain.y, torque.tau);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(load.flange, inertia2.flange_b);
  connect(speedSensor.flange, inertia1.flange_b);
end T4d_ptpdirect;
"#;

#[test]
#[ignore = "rumoca structural bug: same as T4d but KinematicPTP output direct"]
fn tier_4d_ptpdirect_kinematic_ptp_direct_setpoint() {
    try_run("T4d-ptpdirect", "T4d_ptpdirect", T4D_PTPDIRECT_SRC).unwrap();
}

/// T5 — full PID_Controller (the original user report).
#[test]
#[ignore = "rumoca structural bug: full Modelica.Blocks.Examples.PID_Controller"]
fn tier_5_full_pid_controller() {
    try_run(
        "T5",
        "Modelica.Blocks.Examples.PID_Controller",
        "model Dummy end Dummy;",
    )
    .unwrap();
}
