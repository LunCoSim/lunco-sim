// Ackermann rover drivetrain: throttle in, common axle torque + steering out.
//
// The OPTIONAL Modelica drive law (`driveLaw = "modelica"` variant on
// `ackermann_rover.usda`): a single first-order electrical/inertial motor lag
// on the shared drive axle, so torque builds and decays over `tau_m` instead
// of stepping. Unlike `RoverDrivetrain.mo` (skid: steer mixes into per-side
// torque), an Ackermann rover steers with its FRONT KNUCKLES, not a torque
// differential — so both sides get the SAME lagged throttle and `steer` passes
// straight through to the steering port. The passthrough matters: the built-in
// Ackermann linear kernel writes drive_left/drive_right/steering, and the
// variant's `lunco:driveKernel = "external"` sentinel stands ALL of it down,
// so this law must feed all three ports or the rover loses steering.
//
// RUMOCA RULES (same as LegStrut.mo): branch-free equations — `der(x) = expr`
// with `max`/`min` clamps only, no `if`/`when`. Compiled by rumoca via
// `lunco:program:sourceAsset`; ports wire natively via `inputs:x.connect`.
//
// The outputs are NORMALIZED commands (−1..1) so the rhai bridge
// (`assets/scenarios/rover_modelica_ackermann_drive.rhai`) can write them
// straight onto the FSW ports, which expect normalized values.

model RoverAckermannDrivetrain
  parameter Real tau_m = 0.15 "Motor electrical + inertia lag (s)";

  input Real throttle "Normalized forward command, -1..1";
  input Real steer "Normalized steer command, -1..1 (+right)";

  // Common axle torque state, as a fraction of peak torque.
  Real t(start = 0) "Axle torque fraction";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
  output Real steering "Normalized steering command passthrough, -1..1";
equation
  // First-order lag toward the clamped throttle; steering is geometry, not
  // torque, so it bypasses the motor lag entirely.
  der(t) = (max(-1.0, min(1.0, throttle)) - t) / tau_m;
  drive_left = t;
  drive_right = t;
  steering = steer;
end RoverAckermannDrivetrain;
