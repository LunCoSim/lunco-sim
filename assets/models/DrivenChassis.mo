// tagline: Driven chassis — declarative reference for drive_force_mag + contact_friction
//
// The continuous, proper-solver "ground truth" for the longitudinal drive law
// `lunco-mobility::drive_force_mag` working against `contact_friction`. A chassis of
// mass m is pushed by a constant throttle and opposed by contact friction; it
// accelerates to a terminal velocity where the two balance.
//
// Compare against the in-repo RK4 reference + production law in the `oracle` test
// module of `crates/lunco-mobility/src/lib.rs`:
//   * throttle = 0.2 → balanced terminal velocity v_term = drive/k = 19.62 m/s
//     (`drive_accelerates_to_a_balanced_terminal_velocity`),
//   * throttle = -0.2 → exact mirror (`reverse_throttle_mirrors_forward`),
//   * throttle = 0.8 → drive exceeds the cone μ·N → wheelspin, the chassis keeps
//     accelerating, net accel → (drive − μ·N)/m
//     (`excess_throttle_breaks_traction_past_the_friction_cone`).
//
// drive = clamp(throttle, -1, 1) · N · DEFAULT_DRIVE_FORCE_PER_NORMAL (2.0). Friction
// is continuous through zero (linear -k·v below the cone, saturating at -μ·N), as in
// SlidingBlock.mo. Parameters mirror the oracle (m = 250, N = m·g, grip k = 50).
model DrivenChassis
  parameter Real m = 250.0          "Chassis mass (kg) — quarter of a 1000 kg chassis";
  parameter Real throttle = 0.2     "Normalized throttle in [-1, 1]";
  parameter Real n_normal = 2452.5  "Contact normal force (N) = m·g";
  parameter Real drive_per_normal = 2.0 "DEFAULT_DRIVE_FORCE_PER_NORMAL";
  parameter Real k = 50.0           "Contact grip stiffness (N per m/s)";
  parameter Real mu_n = 2452.5      "Coulomb friction cone μ·N (N) = m·g with μ = 1";

  Real v(start = 0.0)               "Longitudinal velocity (m/s)";
  output Real f_drive               "Drive force (N) — compare to drive_force_mag(throttle, N)";
  output Real f_fric                "Friction force (N) — compare to contact_friction(...).x";
equation
  // drive_force_mag: throttle clamped to [-1, 1] (negative = reverse), times N·2.
  f_drive = max(-1.0, min(1.0, throttle)) * n_normal * drive_per_normal;
  // Continuous-through-zero contact friction: -k·v clamped to the cone (same law
  // as SlidingBlock.mo).
  f_fric = -max(-mu_n, min(mu_n, k * v));
  m * der(v) = f_drive + f_fric;
  annotation(experiment(StopTime = 30.0, Interval = 0.001));
end DrivenChassis;
