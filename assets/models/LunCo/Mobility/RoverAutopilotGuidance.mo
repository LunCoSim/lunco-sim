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
// Skid steering can turn in place. Wheel-steered vehicles instead follow the
// target curvature and reverse when the target is behind them; zero drive with
// a steering angle cannot recover a missed waypoint on an Ackermann chassis.
model RoverAutopilotGuidance
  extends LunCo.Icons.Guidance;

  input Real enabled = 0.0 "1 while waypoint guidance is active";
  input Real target_x = 0.0 "Waypoint X in the active physics frame (m)";
  input Real target_z = 0.0 "Waypoint Z in the active physics frame (m)";
  input Real position_x = 0.0 "Authoritative rover position X (m)";
  input Real position_z = 0.0 "Authoritative rover position Z (m)";
  input Real yaw = 0.0 "Authoritative rover yaw (rad)";
  input Real yaw_rate = 0.0 "Authoritative rover angular rate about Y (rad/s)";
  input Real velocity_x = 0.0 "Authoritative rover velocity X (m/s)";
  input Real velocity_z = 0.0 "Authoritative rover velocity Z (m/s)";
  input Real speed = 0.0 "Signed skid-steer cruise limit; wheel steering uses the magnitude";
  input Real radius = 2.0 "Horizontal arrival radius (m)";
  input Real turn_only = 0.0 "1 for a steering-only heading maneuver";

  parameter Real drive_heading_tolerance = 0.2
    "Skid-steer heading window before forward travel (rad)";
  parameter Real maximum_yaw_rate = 0.12 "Skid-steer turn-rate limit (rad/s)";
  parameter Real turn_rate_kp = 3.0 "Turn-rate proportional gain";
  parameter Real turn_rate_ki = 0.8 "Turn-rate integral gain";
  parameter Real heading_kp = 1.2 "Heading proportional gain (1/rad)";
  parameter Real heading_kd = 0.8 "Yaw-rate damping gain (s/rad)";
  parameter Real front_transition = 0.05
    "Continuous front/behind transition in heading cosine";
  parameter Real turn_in_place = 1.0
    "1 for skid steering, 0 for wheel-steered forward/reverse recovery";

  output Real throttle_cmd "Signed drive plus pivot-hold correction";
  output Real travel_throttle_cmd "Signed waypoint travel command";
  output Real pivot_hold_throttle "Common motor demand holding translation during a pivot";
  output Real steer_cmd "Damped normalized steer command";
  output Real target_distance_m(unit = "m") "Horizontal target distance";
  output Real heading_error_rad(unit = "rad")
    "Signed right-positive heading error";
  output Real alignment "Forward alignment cosine";

  Real dx;
  Real dz;
  Real distance_safe;
  Real requested_drive_direction;
  Real target_x_unit;
  Real target_z_unit;
  Real forward_x;
  Real forward_z;
  Real cross_yaw;
  Real dot_heading;
  Real front_gate;
  Real skid_alignment_gate;
  Real approach_gate;
  Real enabled_gate;
  Real turn_only_gate;
  Real steer_gate;
  Real raw_steer;
  Real desired_right_yaw_rate;
  LunCo.GNC.PIDAxis turn_rate_controller(
    enabled=enabled_gate * turn_in_place * steer_gate,
    setpoint=desired_right_yaw_rate,
    measurement=-yaw_rate,
    kp=turn_rate_kp,
    ki=turn_rate_ki,
    kd=0.0,
    integral_limit=1.0 / max(1.0e-9, turn_rate_ki),
    output_limit=1.0,
    anti_windup_gain=2.0);
  Real pivot_gate;
  LunCo.GNC.PIDAxis pivot_hold_controller(
    enabled=enabled_gate * turn_in_place * pivot_gate,
    setpoint=0.0,
    measurement=velocity_x * forward_x + velocity_z * forward_z,
    kp=2.0,
    ki=1.0,
    kd=0.0,
    integral_limit=0.3,
    output_limit=0.3,
    anti_windup_gain=2.0);
  Real wheel_steer;
  Real travel_direction;
  Real arrival_gate;
  Real travel_approach;

equation
  assert(drive_heading_tolerance > 0.0 and drive_heading_tolerance <= 1.5707963267948966,
    "Rover guidance heading window must be in (0, pi/2]");
  assert(maximum_yaw_rate > 0.0 and turn_rate_kp > 0.0 and turn_rate_ki > 0.0,
    "Rover turn-rate limits and gains must be positive");
  enabled_gate = max(0.0, min(1.0, enabled));
  turn_only_gate = max(0.0, min(1.0, turn_only));

  dx = target_x - position_x;
  dz = target_z - position_z;
  distance_safe = sqrt(max(1.0e-12, dx * dx + dz * dz));
  // Reverse skid travel keeps the chassis aimed opposite the travel vector.
  // Steering still controls the physical chassis yaw; signed motor demand
  // supplies reverse motion through the same electrical and wheel models.
  requested_drive_direction = noEvent(if turn_in_place > 0.5 and speed < 0.0 then -1.0 else 1.0);
  target_x_unit = requested_drive_direction * dx / distance_safe;
  target_z_unit = requested_drive_direction * dz / distance_safe;

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
  // Skid steering aligns before translating. A continuous cosine window
  // reduces forward drive while the heading error grows, so a pivot does
  // not become a wide powered arc. Wheel-steered guidance retains curvature.
  skid_alignment_gate = max(0.0, min(1.0,
    (dot_heading - cos(drive_heading_tolerance)) /
      max(1.0e-9, 1.0 - cos(drive_heading_tolerance))));
  approach_gate = max(0.0, min(1.0,
    (distance_safe - max(1.0e-3, radius)) /
      max(1.0e-3, 2.0 * max(1.0e-3, radius))));
  steer_gate = (1.0 - turn_only_gate) * arrival_gate + turn_only_gate;

  // The outer heading loop requests a bounded right-positive yaw rate.
  // Avian Y rate is left-positive, so its negation is the measured right rate.
  // The shared PI axis supplies the friction-holding bias without sacrificing
  // the rate limit or accumulating integral while guidance is disabled.
  desired_right_yaw_rate = max(-maximum_yaw_rate,
    min(maximum_yaw_rate, heading_kp * heading_error_rad));
  raw_steer = turn_rate_controller.command;
  travel_direction = 2.0 * front_gate - 1.0;
  // Curvature has the same sign in forward and reverse. Rate damping changes
  // sign with travel direction because steering reverses its yaw effect.
  wheel_steer = heading_kp * sin(heading_error_rad)
    + heading_kd * yaw_rate * travel_direction;
  // The crawl-to-stop band belongs inside the accepted arrival circle.
  // Tapering outside it can balance a slope before the waypoint is reached.
  arrival_gate = max(0.0, min(1.0,
    (distance_safe - max(1.0e-3, radius) + 0.05) / 0.05));
  travel_approach = arrival_gate * (0.35 + 0.65 * approach_gate);
  steer_cmd = enabled_gate * (
    turn_in_place * steer_gate * max(-1.0, min(1.0, raw_steer))
    + (1.0 - turn_in_place) * arrival_gate *
      max(-1.0, min(1.0, wheel_steer)));

  // A pivot still needs common motor torque to resist rolling down a slope.
  // Physical velocity feedback supplies that torque; it never moves the pose.
  pivot_gate = max(turn_only_gate, 1.0 - skid_alignment_gate);
  pivot_hold_throttle = pivot_hold_controller.command;
  throttle_cmd = travel_throttle_cmd + pivot_hold_throttle;
  travel_throttle_cmd = enabled_gate * (1.0 - turn_only_gate) * (
    turn_in_place * speed * front_gate * skid_alignment_gate *
      (0.25 + 0.75 * max(0.0, dot_heading)) * travel_approach
    + (1.0 - turn_in_place) * abs(speed) * travel_direction * travel_approach);

end RoverAutopilotGuidance;
