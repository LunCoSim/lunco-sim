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
  Real guidance_gate "Unpossessed waypoint-guidance authority";
  Real selected_throttle;
  Real selected_steer;

  // Per-side torque states, as fractions of peak torque.
  Real tl(start = 0) "Left-side torque fraction";
  Real tr(start = 0) "Right-side torque fraction";

  output Real drive_left "Normalized left-side drive, -1..1";
  output Real drive_right "Normalized right-side drive, -1..1";
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
  guidance_gate = (1.0 - piloted_gate) * max(0.0, min(1.0, autopilot_enable));
  guidance.enabled = guidance_gate;
  selected_throttle = piloted_gate * throttle +
    (1.0 - piloted_gate) * guidance.throttle_cmd;
  selected_steer = piloted_gate * steer +
    (1.0 - piloted_gate) * guidance.steer_cmd;

  // First-order lag toward the authored skid law. `steer` adds on the left
  // and subtracts on the right, so +steer yaws right.
  der(tl) = (max(-1.0, min(1.0,
    selected_throttle + steer_gain * selected_steer)) - tl) / tau_m;
  der(tr) = (max(-1.0, min(1.0,
    selected_throttle - steer_gain * selected_steer)) - tr) / tau_m;
  drive_left = tl;
  drive_right = tr;
  guidance_throttle = guidance.throttle_cmd;
  guidance_steer = guidance.steer_cmd;
  guidance_heading_error = guidance.heading_error_rad;
end RoverDrivetrain;
