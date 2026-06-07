// tagline: Sliding block + ground friction — declarative reference for contact_friction
//
// The continuous, proper-solver "ground truth" for the longitudinal friction law
// `lunco-mobility::contact_friction`. A block of mass m slides on flat ground with
// initial velocity and decelerates under contact friction until it comes to rest.
//
// The friction is **continuous through zero** (the fix for the steering-jitter
// stiction limit-cycle): linear `-k·v` below the Coulomb cone, saturating at `-μ·N`
// above it. Compare against the in-repo RK4 reference + production law in the
// `oracle` test module of `crates/lunco-mobility/src/lib.rs`
// (`friction_brings_a_sliding_block_to_rest_without_chatter`). An adaptive Modelica
// solver brings it smoothly to rest; the old slip dead-band would chatter near zero
// (see `deadband_friction_chatters_the_regression_the_fix_removed`).
//
// Parameters mirror the oracle: m = quarter chassis (250 kg), grip k = 600 N·s/m,
// cone μ·N = m·g (μ = 1). Knee μ·N/k ≈ 4.1 m/s, so v0 = 15 starts in the saturated
// Coulomb regime, decelerates linearly, then asymptotes through the viscous knee.
model SlidingBlock
  parameter Real m = 250.0    "Block mass (kg) — quarter of a 1000 kg chassis";
  parameter Real k = 600.0    "Contact grip stiffness (N per m/s) — contact_grip_stiffness";
  parameter Real mu_n = 2452.5 "Coulomb friction cone μ·N (N) = m·g with μ = 1";

  Real v(start = 15.0)        "Longitudinal velocity (m/s)";
  output Real f_fric          "Friction force (N) — compare to contact_friction(...).x";
equation
  // Continuous-through-zero: -k·v clamped to the Coulomb cone [-μ·N, μ·N] — linear
  // inside, saturating at the cone. No dead-band — that is the stiction-jitter
  // regression this reference guards against.
  f_fric = -max(-mu_n, min(mu_n, k * v));
  m * der(v) = f_fric;
  annotation(experiment(StopTime = 4.0, Interval = 0.001));
end SlidingBlock;
