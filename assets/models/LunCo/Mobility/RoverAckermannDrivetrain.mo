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
  input Real autopilot_enable "1 while Modelica waypoint guidance owns the drive";
  input Real autopilot_target_x "Waypoint X in the active physics frame (m)";
  input Real autopilot_target_z "Waypoint Z in the active physics frame (m)";
  input Real autopilot_speed "Forward throttle limit";
  input Real autopilot_radius "Horizontal arrival radius (m)";
  input Real autopilot_turn_only "1 for a steering-only heading maneuver";
  input Real autopilot_position_x "Authoritative rover position X (m)";
  input Real autopilot_position_z "Authoritative rover position Z (m)";
  input Real autopilot_yaw "Authoritative rover yaw (rad)";
  input Real autopilot_yaw_rate "Authoritative rover angular rate about Y (rad/s)";
  input Real piloted "1 while an external session owns manual control";

  RoverAutopilotGuidance guidance;
  Real piloted_gate "Clamped possession signal";
  Real selected_throttle;
  Real selected_steer;

  // Common axle torque state, as a fraction of peak torque.
  Real t(start = 0) "Axle torque fraction";
  Real heading_command "Signed joint heading command (rad)";
  Real tangent_heading "Tangent of the signed joint heading";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
  output Real heading_fl "Final left-front joint heading (rad)";
  output Real heading_fr "Final right-front joint heading (rad)";
  output Real guidance_throttle "Modelica waypoint throttle command";
  output Real guidance_steer "Modelica waypoint steer command";
  output Real guidance_heading_error "Modelica waypoint heading error (rad)";
equation
  guidance.target_x = autopilot_target_x;
  guidance.target_z = autopilot_target_z;
  guidance.position_x = autopilot_position_x;
  guidance.position_z = autopilot_position_z;
  guidance.yaw = autopilot_yaw;
  guidance.yaw_rate = autopilot_yaw_rate;
  guidance.speed = autopilot_speed;
  guidance.radius = autopilot_radius;
  guidance.turn_only = autopilot_turn_only;

  piloted_gate = max(0.0, min(1.0, piloted));
  guidance.enabled = (1.0 - piloted_gate) * max(0.0, min(1.0, autopilot_enable));
  selected_throttle = piloted_gate * throttle +
    (1.0 - piloted_gate) * guidance.throttle_cmd;
  selected_steer = piloted_gate * steer +
    (1.0 - piloted_gate) * guidance.steer_cmd;

  // First-order lag toward the clamped throttle; heading is geometry, not
  // torque, so it bypasses the motor lag entirely.
  der(t) = (max(-1.0, min(1.0, selected_throttle)) - t) / tau_m;
  drive_left = t;
  drive_right = t;
  heading_command = -max(-1.0, min(1.0, selected_steer)) * max_heading;
  tangent_heading = tan(heading_command);
  heading_fl = atan2(wheelbase * tangent_heading,
                     wheelbase - 0.5 * track * tangent_heading);
  heading_fr = atan2(wheelbase * tangent_heading,
                     wheelbase + 0.5 * track * tangent_heading);
  guidance_throttle = guidance.throttle_cmd;
  guidance_steer = guidance.steer_cmd;
  guidance_heading_error = guidance.heading_error_rad;
end RoverAckermannDrivetrain;
