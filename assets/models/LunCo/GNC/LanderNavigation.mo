within LunCo.GNC;

// Small reusable navigation state for the lander controller.
//
// Flight software does not read Avian position/velocity directly.  It receives
// the altimeter range and IMU local-frame acceleration ports, then propagates a
// local navigation estimate.  The initial state is a mission initialization
// value, not a per-tick truth read; after start, the estimate evolves only from
// sensor telemetry.
model LanderNavigation
  extends LunCo.Icons.Sensor;

  input Real altimeter_range = 0.0 "Range sensor reading to the surface (m)";
  input Real altimeter_range_rate = 0.0
    "Measured range rate (m/s), positive when the vehicle climbs";
  input Real altimeter_valid = 0.0
    "1 when the range measurement hit terrain, 0 when it is out of range";
  input Real imu_coordinate_accel_local_x = 0.0 "IMU local coordinate acceleration X (m/s²)";
  input Real imu_coordinate_accel_local_y = 0.0 "IMU local coordinate acceleration Y (m/s²)";
  input Real imu_coordinate_accel_local_z = 0.0 "IMU local coordinate acceleration Z (m/s²)";

  input Real initial_pos_x = 0.0 "Mission-initialized X position (m)";
  input Real initial_pos_y = 0.0 "Mission-initialized Y position (m)";
  input Real initial_pos_z = 0.0 "Mission-initialized Z position (m)";
  input Real initial_vel_x = 0.0 "Mission-initialized X velocity (m/s)";
  input Real initial_vel_y = 0.0 "Mission-initialized Y velocity (m/s)";
  input Real initial_vel_z = 0.0 "Mission-initialized Z velocity (m/s)";
  input Real altimeter_mount_offset = 3.3 "Sensor-to-COM offset along +Y (m)";
  input Real vertical_velocity_correction_gain = 2.0
    "Range-rate correction gain for the vertical estimator (1/s)";

  output Real nav_pos_x(unit = "m") "Estimated X position";
  output Real nav_pos_y(unit = "m") "Estimated Y position from altimeter";
  output Real nav_pos_z(unit = "m") "Estimated Z position";
  output Real nav_vel_x(unit = "m/s") "Estimated X velocity";
  output Real nav_vel_y(unit = "m/s") "Estimated Y velocity";
  output Real nav_vel_z(unit = "m/s") "Estimated Z velocity";
  output Real measured_altitude(unit = "m") "Raw altimeter measurement";

  Real nav_pos_y_integrated(start = 0.0)
    "Vertical position propagated while the raw ray is invalid";
  Real vertical_position_value "Conditioned vertical position";
  Real measured_altitude_value "Conditioned altitude telemetry";

equation
  // The IMU supplies coordinate acceleration.  Integrate it once for velocity
  // and again for the lateral position estimate.  Vertical position is corrected
  // directly by the range measurement to avoid accumulating altitude drift.
  der(nav_pos_x) = nav_vel_x;
  der(nav_pos_z) = nav_vel_z;
  der(nav_vel_x) = imu_coordinate_accel_local_x;
  // Propagate vertical velocity from the IMU and correct it with measured
  // range-rate when the ray is valid. This is a continuous complementary
  // estimator: it remains sensor-only while avoiding an algebraic switch in the
  // PID feedback path.
  der(nav_vel_y) = imu_coordinate_accel_local_y
    + (if altimeter_valid > 0.5 then
      vertical_velocity_correction_gain * (altimeter_range_rate - nav_vel_y)
      else 0.0);
  // With a downward ray over the landing surface, range and world +Y have the
  // same sign: a climbing vehicle increases the measured distance, while a
  // descending vehicle decreases it. Do not negate this measurement or the
  // derivative term will brake in the wrong direction during descent.
  der(nav_vel_z) = imu_coordinate_accel_local_z;

  der(nav_pos_y_integrated) = nav_vel_y;
  vertical_position_value = if altimeter_valid > 0.5
    then altimeter_range + altimeter_mount_offset
    else nav_pos_y_integrated;
  nav_pos_y = vertical_position_value;
  measured_altitude_value = if altimeter_valid > 0.5 then altimeter_range else 0.0;
  measured_altitude = measured_altitude_value;

initial equation
  nav_pos_x = initial_pos_x;
  nav_pos_y_integrated = initial_pos_y;
  nav_pos_z = initial_pos_z;
  nav_vel_x = initial_vel_x;
  nav_vel_y = initial_vel_y;
  nav_vel_z = initial_vel_z;
end LanderNavigation;
