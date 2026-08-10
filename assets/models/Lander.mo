// tagline: Lander — local-frame flight-control law; physical actuation is composed in USD
model Lander
  "Powered-descent flight-control law. It converts pilot or guidance commands into a local thrust-valve request and a local attitude-torque request. It does not know the vehicle's world pose, engine thrust, propellant tank, nozzle layout, or actuator type."

  // Controller configuration is a live input surface. The USD airframe owns
  // the values, so Inspector edits change the active controller without
  // rewriting source or recompiling the Modelica component. Actuator limits
  // and propellant remain in the USD-composed propulsion networks.
  input Real spool_tau = 0.35
    "Human-stick command filter time constant (s)";
  input Real authority_filter_tau_s = 0.02
    "Attitude-authority filter time constant (s)";
  input Real minimum_time_constant_s = 1.0e-6
    "Smallest permitted controller time constant (s)";
  input Real authority_initial = 0.6
    "Initial filtered attitude authority";
  input Real command_lower_bound = 0.0
    "Lower bound for normalized valve and command inputs";
  input Real command_upper_bound = 1.0
    "Upper bound for normalized valve and command inputs";
  // The rigid-body mass/COM/inertia names belong exclusively to the Avian endpoint on
  // the owning USD prim. Controller gains use a deliberately distinct input
  // surface so one connection cannot be claimed by the Modelica map backend
  // and accidentally prevent Avian from receiving live mass properties.
  input Real controller_inertia_xx = 6250.0;
  input Real controller_inertia_yy = 6250.0;
  input Real controller_inertia_zz = 6250.0;

  // Authority and command sources.
  input Real piloted = 0.0;
  input Real external_throttle = 0.0;
  input Real pitch = 0.0;
  input Real roll = 0.0;
  input Real yaw = 0.0;
  input Real guidance_throttle = 0.0;
  input Real guidance_pitch = 0.0;
  input Real guidance_roll = 0.0;
  input Real guidance_yaw = 0.0;
  input Real ang_authority = 0.6
    "Angular acceleration authority per unit attitude command";
  input Real command_tilt_limit_rad = 0.35
    "Maximum commanded tilt angle for normalized guidance/pilot commands (rad)";

  // Local sensor signals. The sensor/estimator owns frame conversion; this
  // controller sees only body-frame rates and body-frame attitude error.
  input Real gyro_x = 0.0 "Body-frame gyro rate about X (rad/s)";
  input Real gyro_y = 0.0 "Body-frame gyro rate about Y (rad/s)";
  input Real gyro_z = 0.0 "Body-frame gyro rate about Z (rad/s)";
  input Real attitude_error_x = 0.0 "Body-frame upright error about X";
  input Real attitude_error_y = 0.0 "Body-frame upright error about Y";
  input Real attitude_error_z = 0.0 "Body-frame upright error about Z";

  // Each landing leg is its own rigid body joined to the hull by a prismatic
  // suspension joint. The pad collider therefore reports its load through the
  // leg body, not through the hull body. Touchdown is the aggregate of those
  // four native Avian contact forces; it is not inferred from altitude.
  input Real leg_force_px = 0.0 "Native contact load on the +X leg (N)";
  input Real leg_force_nx = 0.0 "Native contact load on the -X leg (N)";
  input Real leg_force_pz = 0.0 "Native contact load on the +Z leg (N)";
  input Real leg_force_nz = 0.0 "Native contact load on the -Z leg (N)";
  input Real leg_contact_px = 0.0 "Native contact state on the +X leg";
  input Real leg_contact_nx = 0.0 "Native contact state on the -X leg";
  input Real leg_contact_pz = 0.0 "Native contact state on the +Z leg";
  input Real leg_contact_nz = 0.0 "Native contact state on the -Z leg";
  input Real upright_axis_y = 1.0
    "Measured body-frame component of navigation up (1 when upright)";

  // Avian publishes rigid-body velocity in the canonical navigation frame.
  // These signals decide whether contact has actually settled; they are not a
  // second navigation estimate and do not replace the sensor-driven GNC state.
  input Real navigation_velocity_x = 0.0
    "Measured navigation-frame X velocity (m/s)";
  input Real navigation_velocity_y = 0.0
    "Measured navigation-frame vertical velocity (m/s)";
  input Real navigation_velocity_z = 0.0
    "Measured navigation-frame Z velocity (m/s)";
  input Real touchdown_ground_speed_mps = 0.5
    "Ground speed below which contact may become settled touchdown (m/s)";
  input Real touchdown_descent_speed_mps = 0.15
    "Vertical speed below which contact may become settled touchdown (m/s)";
  input Real touchdown_settle_tau_s = 0.35
    "Time constant for the contact-to-settled touchdown transition (s)";

  input Real attitude_hold = 0.0
    "Enable local attitude stabilization";
  input Real landing_handoff = 0.0
    "Target-qualified handoff from flight control to the landing gear";
  input Real hold_kp = 2.0
    "Attitude-error gain in angular acceleration units";
  input Real hold_kd = 2.5
    "Body-rate damping gain";
  input Real hold_torque_limit_nm = 6000.0
    "Per-axis attitude torque authority supplied by the composed RCS (N.m)";
  input Real attitude_deadband_rad = 0.01
    "Attitude error below which the stabilizer requests no RCS torque";
  input Real rate_deadband_rad_s = 0.02
    "Body-rate below which the stabilizer requests no RCS torque";

  // Generic actuator demands. `throttle` is a normalized command for the
  // main-engine valve network. Torque is a body-frame request consumed by the
  // USD-composed attitude actuator network.
  output Real throttle "Main-engine valve-opening request, 0..1";
  output Real torque_x "Requested body torque about X (N.m)";
  output Real torque_y "Requested body torque about Y (N.m)";
  output Real torque_z "Requested body torque about Z (N.m)";
  output Real landing_contact
    "Settled four-leg contact while the airframe remains upright";
  output Real touchdown "Touchdown signal from local landing loads";
  output Real desired_tilt_x
    "Requested thrust tilt toward navigation +Z (rad)";
  output Real desired_tilt_z
    "Requested thrust tilt toward navigation -X (rad)";

  Real filter_throttle(start = 0.0);
  Real filter_pitch(start = 0.0);
  Real filter_roll(start = 0.0);
  Real filter_yaw(start = 0.0);
  Real cmd_throttle;
  Real cmd_pitch;
  Real cmd_roll;
  Real cmd_yaw;
  Real live_authority;
  Real command_torque_x;
  Real command_torque_y;
  Real command_torque_z;
  Real hold_torque_x;
  Real hold_torque_y;
  Real hold_torque_z;
  Real hold_error_x;
  Real hold_error_y;
  Real hold_error_z;
  Real hold_rate_x;
  Real hold_rate_y;
  Real hold_rate_z;
  Real all_legs_contact;
  Real upright_contact_gate;
  Real attitude_authority;
  Real ground_speed;
  Real descent_speed;
  Real ground_speed_gate;
  Real descent_speed_gate;
  Real settled_touchdown_target;
  Real settled_touchdown_state(start = 0.0);

initial equation
  // `authority_initial` is part of the live USD input surface, so it has
  // continuous variability and cannot legally appear as a variable's `start`
  // attribute. Sample it in the initialization system instead.
  live_authority = authority_initial;

equation
  // Keep scene-tunable values live for the runtime Modelica interface.
  der(live_authority) = (ang_authority - live_authority)
    / noEvent(max(minimum_time_constant_s, authority_filter_tau_s));
  der(filter_throttle) = (external_throttle - filter_throttle)
    / noEvent(max(minimum_time_constant_s, spool_tau));
  der(filter_pitch) = (pitch - filter_pitch)
    / noEvent(max(minimum_time_constant_s, spool_tau));
  der(filter_roll) = (roll - filter_roll)
    / noEvent(max(minimum_time_constant_s, spool_tau));
  der(filter_yaw) = (yaw - filter_yaw)
    / noEvent(max(minimum_time_constant_s, spool_tau));

  // The possession flag selects the command source; it is not an attitude
  // authority gate. Guidance and pilot commands therefore use the same law.
  cmd_throttle = piloted * filter_throttle
    + (1.0 - piloted) * guidance_throttle;
  cmd_pitch = piloted * filter_pitch
    + (1.0 - piloted) * guidance_pitch;
  cmd_roll = piloted * filter_roll
    + (1.0 - piloted) * guidance_roll;
  cmd_yaw = piloted * filter_yaw
    + (1.0 - piloted) * guidance_yaw;

  throttle = noEvent(max(command_lower_bound,
    min(command_upper_bound, cmd_throttle)));
  // Pitch and roll are ATTITUDE requests, not direct torques.  The old
  // boundary multiplied the normalized guidance value by inertia and applied
  // it as a constant torque while the upright hold loop applied a competing
  // torque.  That is not a cascaded flight controller: it leaves the vehicle
  // tilted while the main engine accelerates it sideways.  Convert the
  // normalized request to a physical tilt target and let the measured
  // attitude/rate loop below close the RCS torque loop.
  desired_tilt_x = cmd_pitch * command_tilt_limit_rad;
  desired_tilt_z = cmd_roll * command_tilt_limit_rad;
  command_torque_x = 0.0;
  command_torque_y = cmd_yaw * controller_inertia_yy * live_authority;
  command_torque_z = 0.0;

  // Stabilization is expressed entirely in the body frame. AttitudeReference
  // converts the commanded navigation-frame thrust direction through the IMU
  // quaternion and emits the signed local error; the IMU emits local gyro rates.
  // RCS is a pulse actuator, not a constant trim motor.  The branch-free
  // dead-zone multiplier keeps small estimator noise from reopening a valve
  // after touchdown while preserving the signed error outside the dead zone.
  // `max` is intentional: the Modelica runtime reconstructs continuous
  // algebraic observables from branch-free expressions.
  hold_error_x = attitude_error_x * noEvent(max(0.0,
    1.0 - attitude_deadband_rad
      / noEvent(max(1.0e-9, abs(attitude_error_x)))));
  hold_error_y = attitude_error_y * noEvent(max(0.0,
    1.0 - attitude_deadband_rad / noEvent(max(1.0e-9, abs(attitude_error_y)))));
  hold_error_z = attitude_error_z * noEvent(max(0.0,
    1.0 - attitude_deadband_rad
      / noEvent(max(1.0e-9, abs(attitude_error_z)))));
  hold_rate_x = gyro_x * noEvent(max(0.0,
    1.0 - rate_deadband_rad_s / noEvent(max(1.0e-9, abs(gyro_x)))));
  hold_rate_y = gyro_y * noEvent(max(0.0,
    1.0 - rate_deadband_rad_s / noEvent(max(1.0e-9, abs(gyro_y)))));
  hold_rate_z = gyro_z * noEvent(max(0.0,
    1.0 - rate_deadband_rad_s / noEvent(max(1.0e-9, abs(gyro_z)))));

  // Native contact is not by itself a landing: a flat surface can catch all
  // four feet while the vehicle is still far from its commanded pad. The
  // flight computer owns the target-qualified handoff, so the RCS remains
  // available until the vehicle is both settled and over the authored zone.
  // This prevents a missed vehicle from losing attitude authority merely
  // because its feet touched some other part of the terrain.
  attitude_authority = attitude_hold * max(0.0, min(1.0,
    1.0 - landing_handoff));
  // Bound the requested torque at the controller/actuator boundary. Without
  // this, a large measured attitude error becomes an impossible torque request
  // that the downstream valve clamp silently clips, invalidating the loop's
  // tuning assumptions and making a recovery sensitive to tiny solver noise.
  hold_torque_x = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_xx
      * (hold_kp * hold_error_x - hold_kd * hold_rate_x)));
  hold_torque_y = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_yy
      * (hold_kp * hold_error_y - hold_kd * hold_rate_y)));
  hold_torque_z = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_zz
      * (hold_kp * hold_error_z - hold_kd * hold_rate_z)));

  torque_x = command_torque_x + hold_torque_x;
  torque_y = command_torque_y + hold_torque_y;
  torque_z = command_torque_z + hold_torque_z;

  // A pad can touch while the hull still has lateral or downward speed. Keep
  // physical four-pad contact separate from settled touchdown so the flight
  // computer stops steering against the leg constraints first, then waits for
  // measured navigation velocities to fall below the landing tolerances. The
  // all-pad gate is sourced only from the native pad colliders; trigger volumes
  // are excluded by the Avian contact primitive.
  ground_speed = sqrt(
    navigation_velocity_x * navigation_velocity_x
      + navigation_velocity_z * navigation_velocity_z);
  descent_speed = abs(navigation_velocity_y);
  ground_speed_gate = max(0.0, min(1.0,
    1.0 - ground_speed
      / max(1.0e-9, touchdown_ground_speed_mps)));
  descent_speed_gate = max(0.0, min(1.0,
    1.0 - descent_speed
      / max(1.0e-9, touchdown_descent_speed_mps)));
  all_legs_contact = max(0.0, min(1.0, leg_contact_px))
    * max(0.0, min(1.0, leg_contact_nx))
    * max(0.0, min(1.0, leg_contact_pz))
    * max(0.0, min(1.0, leg_contact_nz));
  // The up-axis reading comes from the IMU quaternion through the shared
  // AttitudeReference transform. Allow a small contact lean, but reject a
  // side-lying body (body-frame up component <= 0.2).
  upright_contact_gate = max(0.0, min(1.0,
    (upright_axis_y - 0.2) / 0.6));
  settled_touchdown_target = all_legs_contact
    * upright_contact_gate
    * ground_speed_gate * descent_speed_gate;
  // Contact alone is not yet a landing: a vehicle can touch four pads while
  // still translating or descending fast enough to slide across the surface.
  // Keep the flight computer authoritative until the native contact loads and
  // measured body velocity satisfy the same low-speed condition used by the
  // touchdown filter. This lets the engine/RCS dissipate the real residual
  // kinetic energy instead of handing it to an unpowered, sliding airframe.
  landing_contact = settled_touchdown_target;
  der(settled_touchdown_state) = (settled_touchdown_target
    - settled_touchdown_state)
    / max(minimum_time_constant_s, touchdown_settle_tau_s);
  touchdown = max(0.0, min(1.0, settled_touchdown_state));
end Lander;
