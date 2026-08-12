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
  // The prismatic joint state is the landing gear's measured load path. Pad
  // contact can become true at the instant of first touch. The joint states
  // remain measured telemetry, and their rates participate in the quiet-gear
  // qualification; permanent honeycomb crush is not required because a soft
  // touchdown can load all four pads without exceeding material yield.
  input Real leg_displacement_px = 0.0
    "Measured +X suspension displacement along its authored axis (m)";
  input Real leg_displacement_nx = 0.0
    "Measured -X suspension displacement along its authored axis (m)";
  input Real leg_displacement_pz = 0.0
    "Measured +Z suspension displacement along its authored axis (m)";
  input Real leg_displacement_nz = 0.0
    "Measured -Z suspension displacement along its authored axis (m)";
  input Real leg_velocity_px = 0.0
    "Measured +X suspension rate (m/s)";
  input Real leg_velocity_nx = 0.0
    "Measured -X suspension rate (m/s)";
  input Real leg_velocity_pz = 0.0
    "Measured +Z suspension rate (m/s)";
  input Real leg_velocity_nz = 0.0
    "Measured -Z suspension rate (m/s)";
  input Real touchdown_max_suspension_speed_mps = 0.25
    "Maximum measured strut rate before touchdown (m/s)";
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
  input Real touchdown_ground_speed_mps = 0.05
    "Ground speed below which contact may become settled touchdown (m/s)";
  input Real touchdown_descent_speed_mps = 0.05
    "Vertical speed below which contact may become settled touchdown (m/s)";
  input Real touchdown_angular_speed_rad_s = 0.005
    "Body angular speed below which flight authority may pass to the gear (rad/s)";
  input Real engine_cutoff_ground_speed_mps = 0.08
    "Maximum horizontal speed for qualified pad-contact engine cutoff (m/s)";
  input Real engine_cutoff_descent_speed_mps = 0.20
    "Maximum descent speed for qualified pad-contact engine cutoff (m/s)";
  input Real touchdown_min_upright_axis_y = 0.9
    "Minimum measured body-up projection for an upright touchdown";

  input Real attitude_hold = 0.0
    "Enable local attitude stabilization";
  input Real landing_handoff = 0.0
    "Target-qualified handoff from flight control to the landing gear";
  input Real landing_engine_cutoff = 0.0
    "Accepted target-qualified pad-contact cutoff; closes the main engine";
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
    "Loaded, quiet native four-leg contact ready for flight-control handoff";
  output Real engine_cutoff_contact
    "Low-speed native pad contact used to stop adding propulsion energy";
  output Real touchdown
    "Loaded, low-rate suspension touchdown while the airframe remains upright";
  output Real minimum_leg_compression
    "Smallest compression measured across the four native suspension joints (m)";
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
  Real any_leg_contact;
  Real pad_contact_phase;
  Real upright_contact_gate;
  Real attitude_authority;
  Real attitude_position_authority;
  Real ground_speed;
  Real descent_speed;
  Real angular_speed;
  Real ground_speed_gate;
  Real descent_speed_gate;
  Real angular_speed_gate;
  Real engine_cutoff_ground_speed_gate;
  Real engine_cutoff_descent_speed_gate;
  Real maximum_leg_speed;
  Real suspension_rate_gate;
  Real settled_touchdown_target;

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
  command_torque_y = cmd_yaw * controller_inertia_yy * live_authority
    * max(0.0, min(1.0, 1.0 - landing_engine_cutoff));
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

  // Main-engine cutoff and flight-control handoff are separate phases. A
  // qualified pad-contact cutoff closes every propulsion valve. Before that
  // event, the first low-speed contact removes the attitude target term but
  // retains measured-rate damping: RCS may arrest residual rotation, but it
  // cannot lean a grounded vehicle. Cutoff is accepted only after that rate is
  // quiet, so no residual yaw is frozen into the passive landing phase.
  attitude_authority = attitude_hold * max(0.0, min(1.0,
    1.0 - max(landing_handoff, landing_engine_cutoff)));
  attitude_position_authority = max(0.0, min(1.0,
    1.0 - max(landing_engine_cutoff, pad_contact_phase)));
  // Bound the requested torque at the controller/actuator boundary. Without
  // this, a large measured attitude error becomes an impossible torque request
  // that the downstream valve clamp silently clips, invalidating the loop's
  // tuning assumptions and making a recovery sensitive to tiny solver noise.
  hold_torque_x = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_xx
      * (attitude_position_authority * hold_kp * hold_error_x
        - hold_kd * hold_rate_x)));
  hold_torque_y = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_yy
      * (attitude_position_authority * hold_kp * hold_error_y
        - hold_kd * hold_rate_y)));
  hold_torque_z = max(-max(0.0, hold_torque_limit_nm), min(
    max(0.0, hold_torque_limit_nm), attitude_authority * controller_inertia_zz
      * (attitude_position_authority * hold_kp * hold_error_z
        - hold_kd * hold_rate_z)));

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
  angular_speed = sqrt(gyro_x * gyro_x + gyro_y * gyro_y + gyro_z * gyro_z);
  // These are mode predicates, not confidence weights. Multiplying normalized
  // margins made a physically valid quiet landing asymptotically approach a
  // value below the flight computer's touchdown threshold. Each predicate is
  // instead derived directly from an authored tolerance and a measured
  // property; the state filter below supplies the temporal qualification.
  ground_speed_gate = noEvent(if ground_speed
      <= max(0.0, touchdown_ground_speed_mps) then 1.0 else 0.0);
  descent_speed_gate = noEvent(if descent_speed
      <= max(0.0, touchdown_descent_speed_mps) then 1.0 else 0.0);
  angular_speed_gate = noEvent(if angular_speed
      <= max(0.0, touchdown_angular_speed_rad_s) then 1.0 else 0.0);
  engine_cutoff_ground_speed_gate = noEvent(if ground_speed
      <= max(0.0, engine_cutoff_ground_speed_mps) then 1.0 else 0.0);
  engine_cutoff_descent_speed_gate = noEvent(if descent_speed
      <= max(0.0, engine_cutoff_descent_speed_mps) then 1.0 else 0.0);
  all_legs_contact = noEvent(if leg_contact_px >= 0.5
      and leg_contact_nx >= 0.5
      and leg_contact_pz >= 0.5
      and leg_contact_nz >= 0.5 then 1.0 else 0.0);
  any_leg_contact = noEvent(if leg_contact_px >= 0.5
      or leg_contact_nx >= 0.5
      or leg_contact_pz >= 0.5
      or leg_contact_nz >= 0.5 then 1.0 else 0.0);
  // A negative prismatic displacement is compression by the joint's authored
  // axis convention. Keep the minimum as measured evidence and require all
  // four suspension rates to be quiet. Do not require a minimum displacement:
  // an elastoplastic absorber only crushes above yield, so that predicate would
  // reject the physically desirable soft landing.
  minimum_leg_compression = min(
    min(max(0.0, -leg_displacement_px), max(0.0, -leg_displacement_nx)),
    min(max(0.0, -leg_displacement_pz), max(0.0, -leg_displacement_nz)));
  maximum_leg_speed = max(
    max(abs(leg_velocity_px), abs(leg_velocity_nx)),
    max(abs(leg_velocity_pz), abs(leg_velocity_nz)));
  suspension_rate_gate = noEvent(if maximum_leg_speed
      <= max(0.0, touchdown_max_suspension_speed_mps) then 1.0 else 0.0);
  // The up-axis reading comes from the IMU quaternion through the shared
  // AttitudeReference transform. The authored minimum is a direct body-up
  // projection criterion, so a side-lying or inverted hull cannot qualify.
  upright_contact_gate = noEvent(if upright_axis_y
      >= max(-1.0, min(1.0, touchdown_min_upright_axis_y))
      then 1.0 else 0.0);
  // Main-engine cutoff and final flight handoff are different physical events.
  // A qualified low-speed pad switch starts a rate-only contact phase. Once the
  // measured body rate is quiet, cutoff closes propulsion and the passive gear
  // takes the remaining touchdown energy. The event supervisor supplies the
  // contiguous-time qualification; this model publishes only the measured
  // predicate. Final handoff still requires all four pads, low body rates, and
  // quiet struts, so first contact is never mistaken for "settled".
  pad_contact_phase = any_leg_contact
    * upright_contact_gate
    * engine_cutoff_ground_speed_gate * engine_cutoff_descent_speed_gate;
  engine_cutoff_contact = pad_contact_phase * angular_speed_gate;
  landing_contact = all_legs_contact
    * upright_contact_gate * ground_speed_gate * descent_speed_gate
    * angular_speed_gate * suspension_rate_gate;
  settled_touchdown_target = landing_contact;
  // Contact alone is not yet a landing: a vehicle can touch four pads while
  // still translating or descending fast enough to slide across the surface.
  // Once every physical predicate is true, touchdown is an event-ready fact.
  // No low-pass state delays or weakens it: the event layer latches this exact
  // measured transition if later contact bits flicker during solver sleep.
  touchdown = settled_touchdown_target;
end Lander;
