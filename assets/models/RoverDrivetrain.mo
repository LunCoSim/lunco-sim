// Per-side rover drivetrain: throttle + steer in, left/right axle torque out.
//
// The OPTIONAL Modelica drive law (`driveLaw = "modelica"` variant on
// `six_wheel_independent.usda`): a first-order electrical/inertial motor lag on
// each side of a skid drivetrain, so torque builds and decays over `tau_m`
// instead of stepping — the rover leans into a command the way a real motor
// controller lets it. The built-in Rust kernels (skid/linear) remain the
// default; this file exists so a drivetrain can be swapped for a physical
// model by editing USD, with no Rust anywhere (the leg_spring pattern).
//
// RUMOCA RULES (same as LegStrut.mo): branch-free equations — `der(x) = expr`
// with `max`/`min` clamps only, no `if`/`when`. Compiled by rumoca via
// `info:sourceAsset`; ports wire natively via `inputs:x.connect`.
//
// The outputs are NORMALIZED per-side drive commands (−1..1, torque/peak).
// Native USD connections fan them onto wheel drive ports.

model RoverDrivetrain
  parameter Real tau_m = 0.15 "Motor electrical + inertia lag (s)";
  parameter Real steer_gain = 1.0 "Differential authority of steer vs throttle";

  input Real throttle "Normalized forward command, -1..1";
  input Real steer "Normalized steer command, -1..1 (+right)";

  // Per-side torque states, as fractions of peak torque.
  Real tl(start = 0) "Left-side torque fraction";
  Real tr(start = 0) "Right-side torque fraction";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
equation
  // First-order lag toward the clamped skid mix. `steer` adds on the left and
  // subtracts on the right, so +steer yaws right — matching the built-in skid
  // kernel's sign convention.
  der(tl) = (max(-1.0, min(1.0, throttle + steer_gain * steer)) - tl) / tau_m;
  der(tr) = (max(-1.0, min(1.0, throttle - steer_gain * steer)) - tr) / tau_m;
  drive_left = tl;
  drive_right = tr;
end RoverDrivetrain;
