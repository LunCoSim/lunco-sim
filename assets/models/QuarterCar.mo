// tagline: Quarter-car suspension — declarative reference for the Rust wheel force law
//
// The continuous, proper-solver "ground truth" for the suspension physics that
// `lunco-mobility::suspension_force_mag` approximates with a fixed-step explicit
// scheme. A single sprung mass on one spring-damper strut over flat ground, under
// gravity. This is the IDEAL physics (linear spring-damper, no clamp) — the Rust
// production law is a stabilised discrete approximation of it (it caps the damping
// term to stay stable at dt = 1/60). Comparing the two is Step 2 of the
// Modelica-realtime-physics plan (docs/architecture/28-modelica-realtime-physics.md).
//
// Parameters mirror `WheelRaycast::default()` (spring_k = 8000, damping_c = 2800)
// and a quarter of the 1000 kg chassis (m = 250). Equilibrium compression is
// chi_eq = m*g/k = 0.3066 m.
//
// Run via lunica (FastRun / experiment path) to produce the reference trajectory,
// then compare against the in-repo RK4 reference in the `oracle` test module of
// `crates/lunco-mobility/src/lib.rs` (they integrate the SAME equations and should
// agree to many digits for this non-stiff system).
model QuarterCar
  parameter Real m = 250.0   "Sprung mass per wheel (kg) — quarter of a 1000 kg chassis";
  parameter Real k = 8000.0  "Suspension stiffness (N/m) — WheelRaycast.spring_k";
  parameter Real c = 2800.0  "Suspension damping (Ns/m) — WheelRaycast.damping_c";
  parameter Real g = 9.81    "Gravity (m/s^2)";

  Real chi(start = 0.20)     "Suspension compression (m); > 0 while the wheel is in contact";
  Real v(start = 0.0)        "Compression rate (m/s)";
  output Real f_susp         "Suspension normal force (N) — compare to suspension_force_mag";
equation
  // No clamp: this is the physics. The Rust law bounds the damping term to ±spring
  // for fixed-step stability; here the adaptive solver needs no such guard.
  f_susp = if chi > 0 then k * chi + c * v else 0;
  v = der(chi);
  m * der(v) = m * g - f_susp;        // m*chi'' = m*g - F(chi, chi')
  annotation(experiment(StopTime = 3.0, Interval = 0.001));
end QuarterCar;
