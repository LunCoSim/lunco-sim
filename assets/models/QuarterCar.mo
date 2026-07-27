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
  parameter Real chi_band = 1e-6 "Contact-gate width (m) — see the gate below";

  Real chi(start = 0.20)     "Suspension compression (m); > 0 while the wheel is in contact";
  Real v(start = 0.0)        "Compression rate (m/s)";
  output Real f_susp         "Suspension normal force (N) — compare to suspension_force_mag";
  Real contact             "Contact gate, 0..1 — 1 while the wheel is on the ground";
equation
  // No clamp on the force itself: this is the physics. The Rust law bounds the damping
  // term to ±spring for fixed-step stability; here the adaptive solver needs no such
  // guard. What IS needed is the one-sided contact condition — the strut pushes only
  // while compressed and cannot pull the chassis down through the wheel.
  //
  // rumoca is branch-free (`if` in an equation section reconstructs as literal 0, which
  // would zero f_susp and with it the whole trajectory), so the contact condition is a
  // continuous gate: exactly 0 for chi <= 0, exactly 1 for chi >= chi_band. chi_band is
  // sub-micron, far below the 0.2..0.41 m range this experiment actually visits, so the
  // reference trajectory is bit-for-bit the ideal linear spring-damper it was before.
  contact = min(max(chi / chi_band, 0.0), 1.0);
  f_susp = (k * chi + c * v) * contact;
  v = der(chi);
  m * der(v) = m * g - f_susp;        // m*chi'' = m*g - F(chi, chi')
  annotation(experiment(StopTime = 3.0, Interval = 0.001));
end QuarterCar;
