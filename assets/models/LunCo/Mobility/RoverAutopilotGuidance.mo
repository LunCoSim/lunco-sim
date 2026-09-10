within LunCo.Mobility;

// Modelica guidance law for a surface rover.
//
// The mission layer supplies a target and the vehicle's authoritative body
// signals supply position, yaw, and yaw rate.  This model owns the continuous
// heading/turn-rate control math; the behaviour tree only selects the current
// target and its mission mode.  A positive steer command means "turn right"
// on the rover control surface, while Avian's positive Y angular rate is the
// opposite sign for this vehicle frame, hence the additive rate term below.
//
// The front-half gate keeps a rear waypoint from becoming reverse throttle.
// The target remains the heading reference until the body is aligned, so a
// rover turns in place and then drives forward toward the same waypoint.
model RoverAutopilotGuidance
  extends LunCo.Icons.Guidance;

  input Real enabled = 0.0 "1 while waypoint guidance is active";
  input Real target_x = 0.0 "Waypoint X in the active physics frame (m)";
  input Real target_z = 0.0 "Waypoint Z in the active physics frame (m)";
  input Real position_x = 0.0 "Authoritative rover position X (m)";
  input Real position_z = 0.0 "Authoritative rover position Z (m)";
  input Real yaw = 0.0 "Authoritative rover yaw (rad)";
  input Real yaw_rate = 0.0 "Authoritative rover angular rate about Y (rad/s)";
  input Real speed = 0.0 "Forward throttle limit";
  input Real radius = 2.0 "Horizontal arrival radius (m)";
  input Real turn_only = 0.0 "1 for a steering-only heading maneuver";

  parameter Real heading_kp = 1.2 "Heading proportional gain (1/rad)";
  parameter Real heading_kd = 0.8 "Yaw-rate damping gain (s/rad)";
  parameter Real front_transition = 0.05
    "Continuous front/behind transition in heading cosine";

  output Real throttle_cmd "Forward throttle command";
  output Real steer_cmd "Damped normalized steer command";
  output Real brake_cmd "Arrival brake command";
  output Real target_distance_m(unit = "m") "Horizontal target distance";
  output Real heading_error_rad(unit = "rad")
    "Signed right-positive heading error";
  output Real alignment "Forward alignment cosine";

  Real dx;
  Real dz;
  Real distance_safe;
  Real target_x_unit;
  Real target_z_unit;
  Real forward_x;
  Real forward_z;
  Real cross_yaw;
  Real dot_heading;
  Real front_gate;
  Real approach_gate;
  Real enabled_gate;
  Real turn_only_gate;
  Real steer_gate;
  Real raw_steer;

equation
  enabled_gate = max(0.0, min(1.0, enabled));
  turn_only_gate = max(0.0, min(1.0, turn_only));

  dx = target_x - position_x;
  dz = target_z - position_z;
  distance_safe = sqrt(max(1.0e-12, dx * dx + dz * dz));
  target_x_unit = dx / distance_safe;
  target_z_unit = dz / distance_safe;

  // The authored rover frame is local -Z forward.  The Avian yaw output is
  // the same YXZ yaw used by the physical pose boundary.
  forward_x = -sin(yaw);
  forward_z = -cos(yaw);
  cross_yaw = forward_z * target_x_unit - forward_x * target_z_unit;
  dot_heading = forward_x * target_x_unit + forward_z * target_z_unit;

  // Positive means the target is to the rover's right. atan2 keeps the turn
  // direction continuous through the rear half-plane.
  heading_error_rad = atan2(-cross_yaw, dot_heading);
  alignment = dot_heading;
  target_distance_m = distance_safe;

  // A small continuous transition around the lateral plane prevents a solver
  // discontinuity while retaining zero throttle for a decisively rear target.
  front_gate = max(0.0, min(1.0,
    (dot_heading + front_transition) / max(1.0e-9, 2.0 * front_transition)));
  approach_gate = max(0.0, min(1.0,
    (distance_safe - max(1.0e-3, radius)) /
      max(1.0e-3, 2.0 * max(1.0e-3, radius))));
  steer_gate = (1.0 - turn_only_gate) * approach_gate + turn_only_gate;

  // Avian's positive Y rate corresponds to a left turn in the rover's
  // local -Z/+X convention. Adding it to the right-positive heading error is
  // therefore negative feedback and removes the post-turn overshoot.
  raw_steer = heading_kp * heading_error_rad + heading_kd * yaw_rate;
  steer_cmd = enabled_gate * steer_gate *
    max(-1.0, min(1.0, raw_steer));

  throttle_cmd = enabled_gate * (1.0 - turn_only_gate) * front_gate *
    speed * (0.25 + 0.75 * max(0.0, dot_heading)) * approach_gate;
  brake_cmd = enabled_gate * (1.0 - turn_only_gate) * (1.0 - approach_gate);
end RoverAutopilotGuidance;
