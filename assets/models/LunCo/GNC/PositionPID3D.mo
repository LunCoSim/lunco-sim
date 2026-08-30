within LunCo.GNC;

// Three-axis position PID guidance for a powered lander.
//
// The visible Modelica scheme is:
//   IMU + altimeter -> LanderNavigation -> PID X/Y/Z -> thrust vector -> lander
//
// This model owns navigation/control mathematics. USD owns target and sensor
// wiring, while Rhai owns only mission sequencing. There is no per-tick script
// controller and no Modelica Standard Library dependency.
model PositionPID3D
  extends LunCo.Icons.Guidance;

  // Flight-software sensor inputs. These are wired to USD-authoritative sensor
  // ports, never to the rigid body's ground-truth position/velocity ports.
  input Real altimeter_range = 0.0 "Altimeter range to terrain (m)";
  input Real altimeter_position_valid = 0.0
    "1 when the raw ray provides a valid vehicle X/Z observation";
  input Real altimeter_altitude_confidence = 0.0
    "0..1 confidence that the raw ray provides vertical altitude evidence";
  input Real altimeter_vehicle_position_x = 0.0
    "Altimeter-derived vehicle X position (m)";
  input Real altimeter_vehicle_position_y = 0.0
    "Altimeter-derived vehicle Y position (m)";
  input Real altimeter_vehicle_position_z = 0.0
    "Altimeter-derived vehicle Z position (m)";
  input Real imu_coordinate_accel_local_x = 0.0 "IMU local coordinate acceleration X (m/s²)";
  input Real imu_coordinate_accel_local_y = 0.0 "IMU local coordinate acceleration Y (m/s²)";
  input Real imu_coordinate_accel_local_z = 0.0 "IMU local coordinate acceleration Z (m/s²)";
  input Real imu_attitude_quat_w = 1.0 "IMU attitude quaternion W";
  input Real imu_attitude_quat_x = 0.0 "IMU attitude quaternion X";
  input Real imu_attitude_quat_y = 0.0 "IMU attitude quaternion Y";
  input Real imu_attitude_quat_z = 0.0 "IMU attitude quaternion Z";
  parameter Real initial_pos_x = 0.0 "Mission-initialized X navigation state (m)";
  parameter Real initial_pos_y = 0.0 "Mission-initialized Y navigation state (m)";
  parameter Real initial_pos_z = 0.0 "Mission-initialized Z navigation state (m)";
  parameter Real initial_vel_x = 0.0 "Mission-initialized X velocity (m/s)";
  parameter Real initial_vel_y = 0.0 "Mission-initialized Y velocity (m/s)";
  parameter Real initial_vel_z = 0.0 "Mission-initialized Z velocity (m/s)";
  input Real altitude_velocity_correction_gain = 4.0
    "Geometric-altitude velocity observer gain (1/s2)";
  input Real lateral_position_correction_gain = 2.0
    "Terrain-hit lateral position observer gain (1/s)";
  input Real lateral_velocity_correction_gain = 1.0
    "Terrain-hit lateral velocity observer gain (1/s2)";

  // Mission landing target. Its position comes from a kinematic USD target
  // body's live output ports; target velocity is optional and defaults to zero.
  input Real target_x = 0.0 "Landing target X (m)";
  input Real target_y = 5.0 "Landing target Y / vehicle COM height (m)";
  input Real target_z = 0.0 "Landing target Z (m)";
  input Real target_vel_x = 0.0 "Landing target velocity X (m/s)";
  input Real target_vel_y = 0.0 "Landing target velocity Y (m/s)";
  input Real target_vel_z = 0.0 "Landing target velocity Z (m/s)";
  input Real command_tilt_limit_rad = 0.35
    "Airframe maximum component tilt used to bound the requested thrust vector (rad)";

  // Live tuning inputs. These are inputs, not parameters, so USD Inspector edits
  // change the controller state without replacing the Modelica component.
  input Real kp_x = 0.08 "X proportional gain (1/s²)";
  input Real ki_x = 0.003 "X integral gain (1/s³)";
  input Real kd_x = 0.75 "X derivative gain (1/s)";
  input Real kp_y = 0.18 "Y proportional gain (1/s²)";
  input Real ki_y = 0.0 "Y integral gain (1/s³)";
  input Real kd_y = 2.0 "Y derivative gain (1/s)";
  input Real kp_z = 0.08 "Z proportional gain (1/s²)";
  input Real ki_z = 0.003 "Z integral gain (1/s³)";
  input Real kd_z = 0.75 "Z derivative gain (1/s)";
  input Real position_integral_limit = 12.0
    "Integral state limit for all position axes";
  input Real anti_windup_gain = 1.0 "PID anti-windup back-calculation";
  input Real max_lateral_accel = 4.0 "Maximum lateral acceleration (m/s²)";
  input Real max_vertical_accel = 8.0 "Maximum vertical correction (m/s²)";
  input Real descent_speed_limit_mps = 4.0
    "Maximum commanded descent speed above the landing target (m/s)";
  input Real descent_braking_accel_mps2 = 3.0
    "Acceleration used by the stopping-distance descent schedule (m/s²)";
  input Real landing_flare_range_m = 4.0
    "Altimeter range below which guidance maintains hover thrust until touchdown (m)";
  input Real g = 1.62 "Local gravity (m/s²)";
  input Real max_thrust = 60000.0 "Maximum engine thrust (N)";
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg)";
  input Real minimum_positive_mass_kg = 1.0e-6
    "Smallest mass used in acceleration normalization (kg)";
  input Real minimum_vertical_accel_mps2 = 1.0e-6
    "Smallest vertical acceleration used in tilt normalization (m/s²)";
  input Real minimum_thrust_accel_mps2 = 1.0e-6
    "Smallest thrust acceleration used in throttle normalization (m/s²)";
  input Real minimum_engine_alignment = 0.55
    "Minimum upward projection before the main engine may light";

  // The airframe uses normalized guidance commands.
  input Real piloted = 0.0 "1 while a pilot owns the vehicle";
  input Real engage = 1.0 "1 while this mission guidance is active";
  input Real touchdown = 0.0
    "Touchdown state; guidance is removed as the vehicle settles on its legs";
  input Real landing_contact = 0.0
    "Physical four-leg contact; remove flight authority before settled touchdown";
  input Real any_leg_contact = 0.0
    "Native contact on at least one landing leg; starts a missed-target recovery";
  input Real engine_cutoff_contact = 0.0
    "Qualified low-speed pad contact; close propulsion before the gear absorbs touchdown";
  input Real landing_zone_radius_m = 4.0
    "Horizontal target radius required before contact may hand off flight authority (m)";
  input Real landing_handoff_position_radius_m = 0.75
    "Final horizontal error required for the contact handoff (m)";
  input Real predicate_transition_band = 1.0e-3
    "Continuous transition width for landing predicates in their native units";
  input Real landing_handoff_latched = 0.0
    "Latched mission handoff written by the typed touchdown event (0 or 1)";
  input Real landing_engine_cutoff_latched = 0.0
    "Latched target-qualified engine cutoff written by the typed gear-load event (0 or 1)";
  output Real vertical_accel(unit = "m/s2") = vertical_limiter_output
    "Gravity-compensated Y acceleration";
  output Real throttle_cmd "Main-engine command, 0..1";
  output Real pitch_cmd "Lateral tilt command, -1..1";
  output Real roll_cmd "Lateral tilt command, -1..1";
  output Real yaw_cmd "Heading command, -1..1";

  // Evidence channels for the teaching scene.
  output Real target_distance_m(unit = "m") "Distance to landing target";
  output Real measured_altitude(unit = "m") "Altimeter input seen by guidance";
  output Real lateral_accel_x(unit = "m/s2") "PID X acceleration command";
  output Real lateral_accel_z(unit = "m/s2") "PID Z acceleration command";
  output Real position_error_x(unit = "m") "X position error";
  output Real position_error_y(unit = "m") "Y position error";
  output Real position_error_z(unit = "m") "Z position error";
  output Real target_zone_gate
    "Target-proximity gate for the physical contact handoff";
  output Real target_recovery_gate
    "Go-around gate while low and outside the final landing qualification";
  output Real flight_authority_gate
    "Measured guidance authority after pilot and landing handoff gates";
  output Real engine_alignment_gate_output
    "Measured main-engine alignment authority gate";
  output Real landing_handoff
    "Accepted contact handoff mode, latched by the mission event supervisor";
  output Real landing_handoff_request
    "Measured target-qualified contact request for event qualification";
  output Real landing_engine_cutoff
    "Accepted main-engine cutoff mode, latched by the mission event supervisor";
  output Real landing_engine_cutoff_request
    "Measured target-qualified engine-cutoff request for event qualification";
  output Real predicted_landing_x(unit = "m")
    "Ballistic projected impact X from the current navigation state";
  output Real predicted_landing_z(unit = "m")
    "Ballistic projected impact Z from the current navigation state";
  output Real predicted_landing_time(unit = "s")
    "Ballistic projected time to the target-height plane";
  output Real descent_rate_command(unit = "m/s")
    "Sensor-driven descent-rate setpoint from stopping distance";

  // The component instances are intentional: the Modelica diagram shows the
  // sensor icon, guidance icon, and three Logic PID icons as a real scheme.
  LanderNavigation navigation(
    initial_pos_x = initial_pos_x,
    initial_pos_y = initial_pos_y,
    initial_pos_z = initial_pos_z,
    initial_vel_x = initial_vel_x,
    initial_vel_y = initial_vel_y,
    initial_vel_z = initial_vel_z);
  PIDAxis pid_x;
  PIDAxis pid_y;
  PIDAxis pid_z;
  AccelerationLimiter vertical_limiter;

  Real max_thrust_accel;
  Real thrust_accel;
  Real unsaturated_throttle;
  Real pitch_command_raw;
  Real roll_command_raw;
  Real tilt_reference_accel;
  Real thrust_authority;
  Real lateral_accel_magnitude;
  Real lateral_support_vertical_command;
  Real available_lateral_accel;
  Real lateral_scale;
  Real bounded_lateral_accel_x;
  Real bounded_lateral_accel_z;
  Real thrust_vertical_projection;
  Real requested_thrust_accel;
  Real engine_alignment_gate;
  Real engine_full_alignment;
  Real pid_y_command;
  Real altitude_above_target;
  Real lateral_landing_gate;
  Real horizontal_target_error;
  Real landing_handoff_position_gate;
  Real contact_recovery_gate;
  Real missed_target_recovery_gate;
  Real target_contact_engine_gate;
  Real recovery_vertical_command;
  Real projected_time_to_target;
  Real descent_rate_schedule_magnitude;
  Real flare_descent_rate_magnitude;
  Real landing_flare_gate;
  Real vertical_limiter_output;
  Real throttle_command_value;
  Real pitch_command_value;
  Real roll_command_value;
  Real yaw_command_value;
  Real flight_command_gain;
  Real predicate_band;
  LunCo.Sensors.FrameVectorTransform thrust_axis_transform;

equation
  // Rumoca's realtime solver reconstructs continuous equations only. Use one
  // authored transition width for every measured predicate instead of an
  // equation-level `if`, which would be reconstructed as zero. At the authored
  // threshold the ramp is already fully true, preserving the inclusive gate
  // contract; it reaches zero one band beyond the threshold.
  predicate_band = max(1.0e-9, predicate_transition_band);

  // Sensor -> navigation block.
  navigation.altimeter_range = altimeter_range;
  navigation.altimeter_position_valid = altimeter_position_valid;
  navigation.altimeter_altitude_confidence = altimeter_altitude_confidence;
  navigation.altimeter_vehicle_position_x = altimeter_vehicle_position_x;
  navigation.altimeter_vehicle_position_y = altimeter_vehicle_position_y;
  navigation.altimeter_vehicle_position_z = altimeter_vehicle_position_z;
  navigation.imu_coordinate_accel_local_x = imu_coordinate_accel_local_x;
  navigation.imu_coordinate_accel_local_y = imu_coordinate_accel_local_y;
  navigation.imu_coordinate_accel_local_z = imu_coordinate_accel_local_z;
  navigation.imu_attitude_quat_w = imu_attitude_quat_w;
  navigation.imu_attitude_quat_x = imu_attitude_quat_x;
  navigation.imu_attitude_quat_y = imu_attitude_quat_y;
  navigation.imu_attitude_quat_z = imu_attitude_quat_z;
  navigation.gravity_nav_x = 0.0;
  navigation.gravity_nav_y = -g;
  navigation.gravity_nav_z = 0.0;
  navigation.altitude_velocity_correction_gain = altitude_velocity_correction_gain;
  navigation.lateral_position_correction_gain = lateral_position_correction_gain;
  navigation.lateral_velocity_correction_gain = lateral_velocity_correction_gain;

  // Navigation -> PID X/Y/Z. Each axis receives setpoint, feedback, rate, and
  // its own live gains; no axis is a copied or hidden special case.
  pid_x.setpoint = target_x;
  pid_x.measurement = navigation.nav_pos_x;
  // The outer lateral loop is a standard position/velocity PD controller. The
  // navigation observer supplies both terms from the IMU and terrain-return
  // correction; it never reads the rigid-body truth pose. A zero target rate is
  // deliberate: the derivative term brakes the measured cross-range velocity
  // while the proportional term brings the vehicle to the marked pad.
  pid_x.setpoint_rate = target_vel_x;
  pid_x.measurement_rate = navigation.nav_vel_x;
  pid_x.kp = kp_x;
  pid_x.ki = ki_x;
  pid_x.kd = kd_x;
  pid_x.integral_limit = position_integral_limit;
  pid_x.output_limit = max_lateral_accel;
  pid_x.anti_windup_gain = anti_windup_gain;

  pid_y.setpoint = target_y;
  pid_y.measurement = navigation.nav_pos_y;
  // Schedule the vertical rate from the measured navigation altitude.  This
  // is a terminal-descent profile, not a touchdown-speed assertion: it permits
  // about 3 m/s downward above the final approach, then tapers toward about
  // 1 m/s around 10 m above the target. NASA describes the same broad IM-2
  // pattern, while Apollo contact data shows that horizontal speed should be
  // below about 0.73 m/s at first footpad contact. A mission may begin with a
  // larger horizontal disturbance, but the separate lateral PID must remove
  // it before contact.
  //
  // A position-only loop would coast until the target is already below the
  // vehicle, then discover its descent speed too late to brake.  The square
  // root is the physical stopping-distance law v = sqrt(2 a h): it permits a
  // bounded descent high above the pad and automatically reduces the commanded
  // rate as the remaining altitude disappears.  This is a reusable flight-law
  // boundary, not a scene timer or a per-episode target override.
  altitude_above_target = max(0.0, navigation.nav_pos_y - target_y);
  // The stopping-distance law limits the rate high above the pad, while the
  // flare law makes the terminal rate approach zero before the feet touch.
  // Keep lateral braking active through most of the descent unless the vehicle
  // is actually over the authored landing target. Native four-leg contact is a
  // physical fact, not proof that the mission reached its mark: on a flat
  // surface a vehicle can settle far away with four valid contacts. The
  // explicit target radius is the GNC mission contract; it is not inferred
  // from a presentation timer.
  horizontal_target_error = sqrt(
    pid_x.error * pid_x.error + pid_z.error * pid_z.error);
  // The broad zone is a mission event, not a proportional throttle fade. It
  // remains deliberately separate from the final handoff gate below: being
  // close enough to start the terminal approach is not the same as being
  // settled on the marked pad.
  target_zone_gate = max(0.0, min(1.0,
    (max(0.0, landing_zone_radius_m) + predicate_band - horizontal_target_error)
      / predicate_band));
  landing_handoff_position_gate = max(0.0, min(1.0,
    (max(0.0, landing_handoff_position_radius_m) + predicate_band
      - horizontal_target_error) / predicate_band));
  // The continuous flight law publishes a measured transition REQUEST. The
  // event supervisor qualifies it for mission scripts and writes the accepted
  // mode through the typed latch input. That keeps this controller acyclic:
  // contact is a sensor input, while mode ownership remains an event boundary.
  landing_handoff_request = max(0.0, min(1.0,
    (max(landing_handoff_position_gate, landing_engine_cutoff_latched) - 0.5
      + predicate_band) / predicate_band))
    * max(0.0, min(1.0,
      (landing_contact - 0.5 + predicate_band) / predicate_band));
  landing_handoff = max(0.0, min(1.0, landing_handoff_latched));
  // Keep lateral PID authority live until the target-qualified handoff. The
  // position loop itself brings the vehicle to zero lateral error; an altitude
  // fade here would remove that authority before touchdown and let residual
  // drift carry the vehicle away from the mark.
  // First-pad engine cutoff is also the end of translational guidance. With no
  // main-engine thrust, a requested tilt cannot create lateral acceleration;
  // keeping that request alive would ask the RCS to lean a grounded vehicle.
  // The airframe enters rate-only damping at first low-speed pad contact and
  // closes RCS together with the main engine once the cutoff event is accepted.
  // The later four-pad handoff records that the passive gear has settled.
  lateral_landing_gate = max(0.0, min(1.0,
    1.0 - max(landing_handoff, landing_engine_cutoff)));
  // A vehicle that has physically touched down outside the final target must
  // command a real go-around. Use the first native leg contact as the recovery
  // phase boundary; waiting for quiet four-pad contact is circular because the
  // hover thrust and attitude correction keep the vehicle loaded but never
  // allow the final handoff predicate to become true away from the target.
  contact_recovery_gate = max(0.0, min(1.0,
    max(any_leg_contact, max(engine_cutoff_contact, max(touchdown, landing_contact)))));
  missed_target_recovery_gate = max(0.0, min(1.0,
    (1.0 - landing_engine_cutoff)
      * (1.0 - landing_handoff_position_gate) * contact_recovery_gate));
  target_recovery_gate = missed_target_recovery_gate;
  recovery_vertical_command = missed_target_recovery_gate * 2.5 * g;
  // Target-qualified low-speed pad contact requests engine cutoff so the
  // event supervisor can transfer the vehicle's weight into the shock
  // absorbers. The strict first-pad predicate is preferred, while the already
  // quiet four-pad predicate is an equivalent physical qualification: it
  // closes the solver-exchange race where handoff consumes the short-lived
  // first-pad sample. Neither path can request cutoff without native contact
  // or the final target-position gate.
  landing_engine_cutoff_request = max(0.0, min(1.0,
    (landing_handoff_position_gate - 0.5 + predicate_band) / predicate_band))
    * max(
      max(0.0, min(1.0,
        (engine_cutoff_contact - 0.5 + predicate_band) / predicate_band)),
      max(0.0, min(1.0,
        (landing_contact - 0.5 + predicate_band) / predicate_band)));
  landing_engine_cutoff = max(0.0, min(1.0, landing_engine_cutoff_latched));
  target_contact_engine_gate = 1.0 - max(
    max(0.0, min(1.0, (landing_handoff - 0.5 + predicate_band) / predicate_band)),
    max(0.0, min(1.0,
      (landing_engine_cutoff - 0.5 + predicate_band) / predicate_band)));
  // A speed cap by itself would still command the cap at the surface and
  // leave the suspension to remove the vehicle's descent energy.
  flare_descent_rate_magnitude = max(0.0, descent_speed_limit_mps)
    * max(0.0, min(1.0,
      altitude_above_target / max(1.0e-9, landing_flare_range_m)));
  descent_rate_schedule_magnitude = min(
    max(0.0, descent_speed_limit_mps),
    min(sqrt(2.0 * max(0.0, descent_braking_accel_mps2)
      * altitude_above_target), flare_descent_rate_magnitude));
  descent_rate_command = target_vel_y - descent_rate_schedule_magnitude;
  pid_y.setpoint_rate = descent_rate_command;
  pid_y.measurement_rate = navigation.nav_vel_y;
  pid_y.kp = kp_y;
  pid_y.ki = ki_y;
  pid_y.kd = kd_y;
  pid_y.integral_limit = position_integral_limit;
  pid_y.output_limit = max_vertical_accel;
  pid_y.anti_windup_gain = anti_windup_gain;

  pid_z.setpoint = target_z;
  pid_z.measurement = navigation.nav_pos_z;
  pid_z.setpoint_rate = target_vel_z;
  pid_z.measurement_rate = navigation.nav_vel_z;
  pid_z.kp = kp_z;
  pid_z.ki = ki_z;
  pid_z.kd = kd_z;
  pid_z.integral_limit = position_integral_limit;
  pid_z.output_limit = max_lateral_accel;
  pid_z.anti_windup_gain = anti_windup_gain;

  max_thrust_accel = max_thrust
    / max(minimum_positive_mass_kg, vehicle_mass);
  // PIDAxis owns saturation. Keep this boundary as a direct signal connection
  // so the parent does not create a second, redundant limiter around the
  // reusable controller's public command.
  lateral_accel_x = lateral_landing_gate * pid_x.command;
  lateral_accel_z = lateral_landing_gate * pid_z.command;
  // Keep the bounded internal control signal separate from the evidence output.
  // This makes the limiter a proper signal boundary and prevents the output
  // alias from participating in the PID algebraic matching row.
  pid_y_command = pid_y.command;
  // Close to the ground, a vertical PID can legitimately request zero thrust
  // while the measured body is still rising from a first pad contact.  That
  // leaves the remaining pads to catch a moving vehicle and makes horizontal
  // settling depend on contact order.  The reusable flare supplies a minimum
  // hover command until native touchdown is settled: it is not a timer and it
  // disappears when the four-leg/upright/low-speed touchdown state is true.
  // It must be keyed to the same target-relative altitude as the stopping law.
  // Using raw altimeter range here mixes the sensor mount/beam geometry with
  // the COM landing datum and can hold a vehicle above its own pad forever.
  landing_flare_gate = max(0.0, min(1.0, altimeter_altitude_confidence))
    * max(0.0, min(1.0,
      altitude_above_target
        / max(1.0e-9, landing_flare_range_m)));
  // A lateral acceleration request is a thrust-vector request, not an
  // independent force channel.  Allowing the vertical law to coast at zero
  // while lateral demand is nonzero makes the later `thrust_authority`
  // multiplier erase the entire thrust vector, so an initial cross-range
  // velocity can never be braked.  Keep the vehicle's normal hover thrust
  // component active while lateral demand exists; the tilt-envelope projection
  // below then limits lateral acceleration to the amount that this hover thrust
  // can realize.  Solving lateral/tan(tilt) here would add upward acceleration
  // to a channel whose value is the gravity-compensating thrust component.
  lateral_support_vertical_command = g * max(0.0, min(1.0,
    lateral_accel_magnitude / max(minimum_vertical_accel_mps2,
      lateral_accel_magnitude)));
  vertical_limiter.command = max(
    target_contact_engine_gate * (g + pid_y_command),
    landing_flare_gate
      * g * landing_handoff_position_gate * (1.0 - landing_handoff)
      * target_contact_engine_gate,
    recovery_vertical_command,
    lateral_support_vertical_command);
  vertical_limiter.lower_limit = 0.0;
  vertical_limiter.upper_limit = max_vertical_accel;
  vertical_limiter_output = vertical_limiter.bounded_command;
  // A zero vertical command means the descent law is deliberately coasting;
  // it must not become a lateral-only engine command. RCS may still pre-align
  // the body for the next burn, but the main engine stays closed until the
  // vertical flight law asks for positive thrust.
  thrust_authority = vertical_limiter_output
    / max(minimum_vertical_accel_mps2, vertical_limiter_output);
  requested_thrust_accel = thrust_authority * sqrt(
    bounded_lateral_accel_x * bounded_lateral_accel_x
      + vertical_limiter_output * vertical_limiter_output
      + bounded_lateral_accel_z * bounded_lateral_accel_z);
  // The commanded vector assumes the body +Y axis has already reached the
  // requested attitude.  During a real RCS transient it has not: the IMU
  // reports a smaller upward projection, so the same throttle would deliver
  // less vertical acceleration.  Compensate that measured loss when the
  // engine is inside its safe alignment envelope.  This is the ordinary
  // thrust-vector guidance relationship F_y = |F| cos(theta), not a
  // presentation correction or a per-vehicle override.  The throttle limit
  // still bounds the result when the vehicle cannot make the requested force.
  thrust_accel = max(requested_thrust_accel,
    vertical_limiter_output
      / max(minimum_engine_alignment, thrust_vertical_projection));
  unsaturated_throttle = thrust_accel
    / max(minimum_thrust_accel_mps2, max_thrust_accel);

  // Do not fire a main engine into the horizon while the airframe is recovering
  // from a large attitude error.  The body +Y axis is the engine axis, so the
  // measured body-to-navigation quaternion gives its upward component directly.
  // This is a flight-computer authority limit, not a scene override: at 90 deg
  // the engine is off, during recovery it ramps with the real thrust-vector
  // projection, and at an upright attitude the normal throttle command is unchanged.
  thrust_axis_transform.quaternion_w = imu_attitude_quat_w;
  thrust_axis_transform.quaternion_x = imu_attitude_quat_x;
  thrust_axis_transform.quaternion_y = imu_attitude_quat_y;
  thrust_axis_transform.quaternion_z = imu_attitude_quat_z;
  thrust_axis_transform.vector_x = 0.0;
  thrust_axis_transform.vector_y = 1.0;
  thrust_axis_transform.vector_z = 0.0;
  thrust_vertical_projection = noEvent(max(0.0, min(1.0,
    thrust_axis_transform.world_frame_y)));
  // A vehicle released on its side must first use RCS to turn its thrust axis
  // toward the surface normal. Multiplying by the raw projection alone would
  // still allow a sideways engine to burn while the projection is small; that
  // spends horizontal fuel before the guidance vector is physically useful.
  // This is a reusable flight-computer safety boundary, expressed from the
  // measured attitude, not a scene timer or a scripted trajectory.
  // The normal commanded tilt envelope is not an unsafe alignment error. Reach
  // full engine authority once the measured axis is inside that envelope, and
  // use the narrow band down to minimum_engine_alignment only as the recovery
  // ramp. Otherwise the more active lateral PID is silently given less vertical
  // thrust, so two vehicles with the same descent law fall at different rates.
  engine_full_alignment = max(minimum_engine_alignment + 1.0e-6,
    cos(max(0.0, command_tilt_limit_rad)));
  engine_alignment_gate = noEvent(max(0.0, min(1.0,
    (thrust_vertical_projection - minimum_engine_alignment)
      / max(1.0e-9, engine_full_alignment - minimum_engine_alignment))));

  // Body +Y is the engine axis. A requested lateral acceleration is physically
  // achievable only when the available vertical thrust can support the
  // airframe's authored tilt limit. The previous boundary divided by `g` even
  // when vertical thrust demand was zero, then asked the airframe for lateral
  // thrust it could only realize as an upward-limited attitude. That converted
  // a descent into an unintended climb and eventually invalidated the altimeter.
  // Bound the VECTOR before converting it to the normalized component-angle
  // command, so the generated thrust and the commanded attitude describe the
  // same physical wrench.
  // Keep a meaningful thrust-vector target while the descent law is coasting.
  // The engine remains gated by `thrust_authority` above; this reference only
  // tells the RCS which attitude will make the next powered correction useful.
  tilt_reference_accel = max(minimum_vertical_accel_mps2,
    max(g, vertical_limiter_output));
  lateral_accel_magnitude = sqrt(
    lateral_accel_x * lateral_accel_x + lateral_accel_z * lateral_accel_z);
  available_lateral_accel = tilt_reference_accel
    * tan(max(1.0e-9, command_tilt_limit_rad));
  lateral_scale = min(1.0, available_lateral_accel
    / max(minimum_vertical_accel_mps2, lateral_accel_magnitude));
  bounded_lateral_accel_x = lateral_accel_x * lateral_scale;
  bounded_lateral_accel_z = lateral_accel_z * lateral_scale;
  pitch_command_raw = atan2(bounded_lateral_accel_z, tilt_reference_accel)
    / max(1.0e-9, command_tilt_limit_rad);
  // The airframe's body +Y thrust axis moves toward navigation -X for a
  // positive body-Z attitude request in this convention. Therefore a
  // negative desired navigation-X acceleration must produce a positive roll
  // command. This sign is the composed vehicle/actuator frame contract, not a
  // scene-specific correction.
  roll_command_raw = -atan2(bounded_lateral_accel_x, tilt_reference_accel)
    / max(1.0e-9, command_tilt_limit_rad);
  // Once native contact is settled over the authored target, flight authority
  // hands off to the suspension. Contact away from the target does not
  // terminate guidance: the vehicle must perform a real go-around or
  // repositioning manoeuvre using the same thrust and sensor loops.
  flight_command_gain = engage * (1.0 - piloted)
    * max(0.0, min(1.0, 1.0 - landing_handoff));
  flight_authority_gate = flight_command_gain;
  engine_alignment_gate_output = engine_alignment_gate;
  // `unsaturated_throttle` is already the magnitude of the requested navigation
  // acceleration vector. When the body axis tracks that vector, multiplying by
  // its vertical projection again applies cos(tilt) twice and under-throttles
  // lateral manoeuvres. The real thrust projection remains in the physics; this
  // command supplies the vector magnitude once, with only the safety gate above.
  throttle_command_value = flight_command_gain * engine_alignment_gate
    * max(0.0, min(1.0, unsaturated_throttle));
  // Throttle is gated by the measured +Y thrust projection because a sideways
  // engine cannot provide useful upward force. Attitude commands are not gated:
  // the RCS must pre-align the thrust vector while the vehicle is recovering,
  // otherwise the first horizontal correction can only start after the vehicle
  // has already spent its descent margin.
  pitch_command_value = flight_command_gain
    * max(-1.0, min(1.0, pitch_command_raw));
  roll_command_value = flight_command_gain
    * max(-1.0, min(1.0, roll_command_raw));
  yaw_command_value = 0.0;
  throttle_cmd = throttle_command_value;
  pitch_cmd = pitch_command_value;
  roll_cmd = roll_command_value;
  yaw_cmd = yaw_command_value;

  // KSP-style landing prediction: project the current measured navigation
  // state under lunar gravity until it reaches the authored COM landing plane.
  // This is deliberately labelled ballistic in the UI. It is not a replayed
  // curve and it does not pretend to know future throttle; the projected point
  // moves as the IMU/altimeter-driven state and the real engine response move.
  projected_time_to_target = max(0.0,
    (navigation.nav_vel_y + sqrt(max(0.0,
      navigation.nav_vel_y * navigation.nav_vel_y
        + 2.0 * max(0.0, g) * max(0.0,
          navigation.nav_pos_y - target_y))))
      / max(1.0e-9, g));
  predicted_landing_time = projected_time_to_target;
  predicted_landing_x = navigation.nav_pos_x
    + navigation.nav_vel_x * projected_time_to_target;
  predicted_landing_z = navigation.nav_pos_z
    + navigation.nav_vel_z * projected_time_to_target;

  target_distance_m = sqrt((target_x - navigation.nav_pos_x)
    * (target_x - navigation.nav_pos_x)
    + (target_y - navigation.nav_pos_y) * (target_y - navigation.nav_pos_y)
    + (target_z - navigation.nav_pos_z) * (target_z - navigation.nav_pos_z));
  measured_altitude = navigation.measured_altitude;
  position_error_x = pid_x.error;
  position_error_y = pid_y.error;
  position_error_z = pid_z.error;
end PositionPID3D;
