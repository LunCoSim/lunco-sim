within LunCo.Mobility;
// Ackermann rover drivetrain: throttle in, common axle torque + steering out.
//
// The model owns the steering geometry. It publishes the final signed heading
// for each front knuckle, so the wheel endpoints receive a physical angle
// rather than a vehicle-class command. The drive state remains a shared
// motor/axle state and is published through generic torque ports.
//
// RUMOCA RULES (same as LegStrut.mo): branch-free equations — `der(x) = expr`
// with `max`/`min` clamps only, no `if`/`when`. Compiled by rumoca via
// `info:sourceAsset`; ports wire natively via `inputs:x.connect`.
//
// Drive outputs are normalized demands (−1..1); heading outputs are radians.
// Authored USD connections publish them onto the generic wheel and joint ports.

model RoverAckermannDrivetrain
  extends LunCo.Icons.Mobility;
  parameter Real tau_m = 0.15 "Motor electrical + inertia lag (s)";
  parameter Real wheelbase = 2.45 "Front/rear axle spacing (m)";
  parameter Real track = 2.0 "Front wheel centre spacing (m)";
  parameter Real max_heading = 0.5 "Maximum front-wheel heading (rad)";

  input Real throttle "Normalized forward command, -1..1";
  input Real steer "Normalized right command, -1..1";

  // Common axle torque state, as a fraction of peak torque.
  Real t(start = 0) "Axle torque fraction";
  Real heading_command "Signed joint heading command (rad)";
  Real tangent_heading "Tangent of the signed joint heading";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
  output Real heading_fl "Final left-front joint heading (rad)";
  output Real heading_fr "Final right-front joint heading (rad)";
equation
  // First-order lag toward the clamped throttle; heading is geometry, not
  // torque, so it bypasses the motor lag entirely.
  der(t) = (max(-1.0, min(1.0, throttle)) - t) / tau_m;
  drive_left = t;
  drive_right = t;
  heading_command = -max(-1.0, min(1.0, steer)) * max_heading;
  tangent_heading = tan(heading_command);
  heading_fl = atan2(wheelbase * tangent_heading,
                     wheelbase - 0.5 * track * tangent_heading);
  heading_fr = atan2(wheelbase * tangent_heading,
                     wheelbase + 0.5 * track * tangent_heading);
end RoverAckermannDrivetrain;
