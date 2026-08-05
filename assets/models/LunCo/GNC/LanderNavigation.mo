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
  input Real imu_attitude_quat_w = 1.0 "IMU attitude quaternion W";
  input Real imu_attitude_quat_x = 0.0 "IMU attitude quaternion X";
  input Real imu_attitude_quat_y = 0.0 "IMU attitude quaternion Y";
  input Real imu_attitude_quat_z = 0.0 "IMU attitude quaternion Z";
  input Real gravity_nav_x = 0.0 "Gravity in the navigation X axis (m/s²)";
  input Real gravity_nav_y = -1.62 "Gravity in the navigation Y axis (m/s²)";
  input Real gravity_nav_z = 0.0 "Gravity in the navigation Z axis (m/s²)";
  parameter Real quaternion_epsilon = 1.0e-12 "Quaternion normalization floor";

  parameter Real initial_pos_x = 0.0 "Mission-initialized X position (m)";
  parameter Real initial_pos_y = 0.0 "Mission-initialized Y position (m)";
  parameter Real initial_pos_z = 0.0 "Mission-initialized Z position (m)";
  parameter Real initial_vel_x = 0.0 "Mission-initialized X velocity (m/s)";
  parameter Real initial_vel_y = 0.0 "Mission-initialized Y velocity (m/s)";
  parameter Real initial_vel_z = 0.0 "Mission-initialized Z velocity (m/s)";
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
  Real q_norm;
  Real q_w;
  Real q_x;
  Real q_y;
  Real q_z;
  Real navigation_accel_x;
  Real navigation_accel_y;
  Real navigation_accel_z;
  Real range_confidence;

equation
  // The IMU supplies specific force in the body frame. Rotate it into the
  // navigation frame with the measured attitude, then restore gravity. The
  // controller never integrates a body-local axis as though it were a
  // navigation axis; the frame conversion remains in the Modelica estimator.
  q_norm = sqrt(max(quaternion_epsilon,
    imu_attitude_quat_w * imu_attitude_quat_w
      + imu_attitude_quat_x * imu_attitude_quat_x
      + imu_attitude_quat_y * imu_attitude_quat_y
      + imu_attitude_quat_z * imu_attitude_quat_z));
  q_w = imu_attitude_quat_w / q_norm;
  q_x = imu_attitude_quat_x / q_norm;
  q_y = imu_attitude_quat_y / q_norm;
  q_z = imu_attitude_quat_z / q_norm;
  navigation_accel_x =
    (1.0 - 2.0 * (q_y * q_y + q_z * q_z)) * imu_coordinate_accel_local_x
      + 2.0 * (q_x * q_y + q_w * q_z) * imu_coordinate_accel_local_y
      + 2.0 * (q_x * q_z - q_w * q_y) * imu_coordinate_accel_local_z
      + gravity_nav_x;
  navigation_accel_y =
    2.0 * (q_x * q_y - q_w * q_z) * imu_coordinate_accel_local_x
      + (1.0 - 2.0 * (q_x * q_x + q_z * q_z)) * imu_coordinate_accel_local_y
      + 2.0 * (q_y * q_z + q_w * q_x) * imu_coordinate_accel_local_z
      + gravity_nav_y;
  navigation_accel_z =
    2.0 * (q_x * q_z + q_w * q_y) * imu_coordinate_accel_local_x
      + 2.0 * (q_y * q_z - q_w * q_x) * imu_coordinate_accel_local_y
      + (1.0 - 2.0 * (q_x * q_x + q_y * q_y)) * imu_coordinate_accel_local_z
      + gravity_nav_z;

  // Integrate navigation-frame acceleration once for velocity and again for the
  // lateral position estimate. Vertical position is corrected directly by the
  // range measurement to avoid accumulating altitude drift.
  der(nav_pos_x) = nav_vel_x;
  der(nav_pos_z) = nav_vel_z;
  der(nav_vel_x) = navigation_accel_x;
  // Propagate vertical velocity from the IMU and correct it with measured
  // range-rate when the ray is valid. This is a continuous complementary
  // estimator: it remains sensor-only while avoiding an algebraic switch in the
  // PID feedback path.
  range_confidence = max(0.0, min(1.0, altimeter_valid));
  der(nav_vel_y) = navigation_accel_y
    + range_confidence * vertical_velocity_correction_gain
      * (altimeter_range_rate - nav_vel_y);
  // With a downward ray over the landing surface, range and world +Y have the
  // same sign: a climbing vehicle increases the measured distance, while a
  // descending vehicle decreases it. Do not negate this measurement or the
  // derivative term will brake in the wrong direction during descent.
  der(nav_vel_z) = navigation_accel_z;

  der(nav_pos_y_integrated) = nav_vel_y;
  vertical_position_value = range_confidence
    * (altimeter_range + altimeter_mount_offset)
    + (1.0 - range_confidence) * nav_pos_y_integrated;
  nav_pos_y = vertical_position_value;
  measured_altitude_value = range_confidence * altimeter_range;
  measured_altitude = measured_altitude_value;

initial equation
  nav_pos_x = initial_pos_x;
  nav_pos_y_integrated = initial_pos_y;
  nav_pos_z = initial_pos_z;
  nav_vel_x = initial_vel_x;
  nav_vel_y = initial_vel_y;
  nav_vel_z = initial_vel_z;
end LanderNavigation;
