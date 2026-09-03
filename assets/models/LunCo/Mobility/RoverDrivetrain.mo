within LunCo.Mobility;
// Per-side rover drivetrain: throttle + steer in, left/right axle torque out.
//
// This is the authored skid-steer law for rover assets. It integrates one
// motor state for each side and publishes solved drive demands to generic wheel
// torque ports. The vehicle composition selects this model through USD; Rust
// only realizes the resulting ports and wheel/contact mechanics.
//
// RUMOCA RULES (same as LegStrut.mo): branch-free equations — `der(x) = expr`
// with `max`/`min` clamps only, no `if`/`when`. Compiled by rumoca via
// `info:sourceAsset`; ports wire natively via `inputs:x.connect`.
//
// The outputs are normalized per-side drive demands (−1..1, torque/peak).
// Authored USD connections fan them onto generic wheel drive ports.

model RoverDrivetrain
  extends LunCo.Icons.Mobility;
  parameter Real tau_m = 0.15 "Motor electrical + inertia lag (s)";
  parameter Real steer_gain = 1.0 "Differential authority of steer vs throttle";

  input Real throttle "Normalized forward command, -1..1";
  input Real steer "Normalized right command, -1..1";

  // Per-side torque states, as fractions of peak torque.
  Real tl(start = 0) "Left-side torque fraction";
  Real tr(start = 0) "Right-side torque fraction";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
equation
  // First-order lag toward the authored skid law. `steer` adds on the left
  // and subtracts on the right, so +steer yaws right.
  der(tl) = (max(-1.0, min(1.0, throttle + steer_gain * steer)) - tl) / tau_m;
  der(tr) = (max(-1.0, min(1.0, throttle - steer_gain * steer)) - tr) / tau_m;
  drive_left = tl;
  drive_right = tr;
end RoverDrivetrain;
