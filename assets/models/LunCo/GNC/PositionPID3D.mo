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
  input Real altimeter_range_rate = 0.0 "Altimeter range rate (m/s)";
  input Real altimeter_valid = 0.0 "1 when the raw ray has a valid return";
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
  input Real altimeter_mount_offset = 3.3 "Altimeter-to-COM offset (m)";
  input Real vertical_velocity_correction_gain = 2.0
    "Range-rate correction gain for the vertical estimator (1/s)";

  // Mission landing target. Its position comes from a kinematic USD target
  // body's live output ports; target velocity is optional and defaults to zero.
  input Real target_x = 0.0 "Landing target X (m)";
  input Real target_y = 5.0 "Landing target Y / vehicle COM height (m)";
  input Real target_z = 0.0 "Landing target Z (m)";
  input Real target_vel_x = 0.0 "Landing target velocity X (m/s)";
  input Real target_vel_y = 0.0 "Landing target velocity Y (m/s)";
  input Real target_vel_z = 0.0 "Landing target velocity Z (m/s)";

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
  input Real g = 1.62 "Local gravity (m/s²)";
  input Real max_thrust = 60000.0 "Maximum engine thrust (N)";
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg)";
  input Real minimum_positive_mass_kg = 1.0e-6
    "Smallest mass used in acceleration normalization (kg)";
  input Real minimum_vertical_accel_mps2 = 1.0e-6
    "Smallest vertical acceleration used in tilt normalization (m/s²)";
  input Real minimum_thrust_accel_mps2 = 1.0e-6
    "Smallest thrust acceleration used in throttle normalization (m/s²)";

  // The airframe uses normalized guidance commands.
  input Real piloted = 0.0 "1 while a pilot owns the vehicle";
  input Real engage = 1.0 "1 while this mission guidance is active";
  input Real touchdown = 0.0
    "Touchdown state; guidance is removed as the vehicle settles on its legs";
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
  Real attitude_quat_norm_sq;
  Real thrust_vertical_projection;
  Real pid_y_command;
  Real vertical_limiter_output;
  Real throttle_command_value;
  Real pitch_command_value;
  Real roll_command_value;
  Real yaw_command_value;
  Real flight_command_gain;

equation
  // Sensor -> navigation block.
  navigation.altimeter_range = altimeter_range;
  navigation.altimeter_range_rate = altimeter_range_rate;
  navigation.altimeter_valid = altimeter_valid;
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
  navigation.altimeter_mount_offset = altimeter_mount_offset;
  navigation.vertical_velocity_correction_gain = vertical_velocity_correction_gain;

  // Navigation -> PID X/Y/Z. Each axis receives setpoint, feedback, rate, and
  // its own live gains; no axis is a copied or hidden special case.
  pid_x.setpoint = target_x;
  pid_x.measurement = navigation.nav_pos_x;
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
  pid_y.setpoint_rate = target_vel_y;
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
  lateral_accel_x = pid_x.command;
  lateral_accel_z = pid_z.command;
  // Keep the bounded internal control signal separate from the evidence output.
  // This makes the limiter a proper signal boundary and prevents the output
  // alias from participating in the PID algebraic matching row.
  pid_y_command = pid_y.command;
  vertical_limiter.command = g + pid_y_command;
  vertical_limiter.lower_limit = 0.0;
  vertical_limiter.upper_limit = max_vertical_accel;
  vertical_limiter_output = vertical_limiter.bounded_command;
  thrust_accel = sqrt(lateral_accel_x * lateral_accel_x
    + vertical_limiter_output * vertical_limiter_output
    + lateral_accel_z * lateral_accel_z);
  unsaturated_throttle = thrust_accel
    / max(minimum_thrust_accel_mps2, max_thrust_accel);

  // Do not fire a main engine into the horizon while the airframe is recovering
  // from a large attitude error.  The body +Y axis is the engine axis, so the
  // measured body-to-navigation quaternion gives its upward component directly.
  // This is a flight-computer authority limit, not a scene override: at 90 deg
  // the engine is off, during recovery it ramps with the real thrust-vector
  // projection, and at an upright attitude the normal throttle command is unchanged.
  attitude_quat_norm_sq = max(minimum_positive_mass_kg,
    imu_attitude_quat_w * imu_attitude_quat_w
      + imu_attitude_quat_x * imu_attitude_quat_x
      + imu_attitude_quat_y * imu_attitude_quat_y
      + imu_attitude_quat_z * imu_attitude_quat_z);
  thrust_vertical_projection = noEvent(max(0.0, min(1.0,
    (1.0 - 2.0 * (imu_attitude_quat_x * imu_attitude_quat_x
      + imu_attitude_quat_z * imu_attitude_quat_z))
      / attitude_quat_norm_sq)));

  // Body +Y is the engine axis. Convert the desired world acceleration vector
  // into bounded tilt requests; the airframe's attitude stabilizer closes the
  // angular loop and turns those requests into torque.
  // A free-falling vehicle can still need a bounded lateral correction. Using
  // `vertical_limiter_output` alone makes that case divide by zero and turn a
  // small position error into a saturated ninety-degree attitude request. The
  // local gravity magnitude is the physical reference for a lateral tilt when
  // vertical thrust demand is below hover; it keeps the command a real thrust
  // vector without inventing a world-frame quantity.
  tilt_reference_accel = max(minimum_vertical_accel_mps2,
    max(g, vertical_limiter_output));
  pitch_command_raw = lateral_accel_z / tilt_reference_accel;
  // The airframe's body +Y thrust axis moves toward navigation -X for a
  // positive body-Z attitude request in this convention. Therefore a
  // negative desired navigation-X acceleration must produce a positive roll
  // command. This sign is the composed vehicle/actuator frame contract, not a
  // scene-specific correction.
  roll_command_raw = -lateral_accel_x / tilt_reference_accel;
  // A landed vehicle must not continue steering against its leg constraints.
  // Touchdown is a measured airframe state, not a Rhai timer or a scene-specific
  // controller branch. The continuous transition from AboveThreshold lets the
  // guidance demand fade out as the load settles.
  flight_command_gain = engage * (1.0 - piloted)
    * max(0.0, min(1.0, 1.0 - touchdown));
  throttle_command_value = flight_command_gain * thrust_vertical_projection
    * max(0.0, min(1.0, unsaturated_throttle));
  pitch_command_value = flight_command_gain
    * max(-1.0, min(1.0, pitch_command_raw));
  roll_command_value = flight_command_gain
    * max(-1.0, min(1.0, roll_command_raw));
  yaw_command_value = 0.0;
  throttle_cmd = throttle_command_value;
  pitch_cmd = pitch_command_value;
  roll_cmd = roll_command_value;
  yaw_cmd = yaw_command_value;

  target_distance_m = sqrt((target_x - navigation.nav_pos_x)
    * (target_x - navigation.nav_pos_x)
    + (target_y - navigation.nav_pos_y) * (target_y - navigation.nav_pos_y)
    + (target_z - navigation.nav_pos_z) * (target_z - navigation.nav_pos_z));
  measured_altitude = navigation.measured_altitude;
  position_error_x = pid_x.error;
  position_error_y = pid_y.error;
  position_error_z = pid_z.error;
end PositionPID3D;
