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
  Real nav_vel_y_integrated(start = 0.0)
    "Vertical velocity propagated while the raw ray is invalid";
  Real vertical_position_value "Conditioned vertical position";
  Real measured_altitude_value "Conditioned altitude telemetry";
  Real navigation_accel_x;
  Real navigation_accel_y;
  Real navigation_accel_z;
  Real range_confidence;
  LunCo.Sensors.FrameVectorTransform acceleration_transform(
    quaternion_epsilon = quaternion_epsilon);

equation
  // The IMU supplies specific force in the body frame. Rotate it into the
  // navigation frame with the measured attitude, then restore gravity. The
  // controller never integrates a body-local axis as though it were a
  // navigation axis; the frame conversion remains in the Modelica estimator.
  acceleration_transform.quaternion_w = imu_attitude_quat_w;
  acceleration_transform.quaternion_x = imu_attitude_quat_x;
  acceleration_transform.quaternion_y = imu_attitude_quat_y;
  acceleration_transform.quaternion_z = imu_attitude_quat_z;
  acceleration_transform.vector_x = imu_coordinate_accel_local_x;
  acceleration_transform.vector_y = imu_coordinate_accel_local_y;
  acceleration_transform.vector_z = imu_coordinate_accel_local_z;
  navigation_accel_x = acceleration_transform.world_frame_x + gravity_nav_x;
  navigation_accel_y = acceleration_transform.world_frame_y + gravity_nav_y;
  navigation_accel_z = acceleration_transform.world_frame_z + gravity_nav_z;

  // Integrate navigation-frame acceleration once for velocity and again for the
  // lateral position estimate. Vertical position is corrected directly by the
  // range measurement to avoid accumulating altitude drift.
  der(nav_pos_x) = nav_vel_x;
  der(nav_pos_z) = nav_vel_z;
  der(nav_vel_x) = navigation_accel_x;
  // The valid range-rate is the direct vertical-velocity measurement. Use it
  // for the control output whenever the altimeter has a real return, and keep
  // the IMU-propagated state as the out-of-range estimate. This is a
  // continuous complementary estimator: the flight computer never mistakes
  // a transient IMU integration state for a measured descent rate while the
  // ray is valid.
  range_confidence = max(0.0, min(1.0, altimeter_valid));
  der(nav_vel_y_integrated) = navigation_accel_y;
  nav_vel_y = range_confidence * altimeter_range_rate
    + (1.0 - range_confidence) * nav_vel_y_integrated;
  // With a downward ray over the landing surface, range and world +Y have the
  // same sign: a climbing vehicle increases the measured distance, while a
  // descending vehicle decreases it. Do not negate this measurement or the
  // derivative term will brake in the wrong direction during descent.
  der(nav_vel_z) = navigation_accel_z;

  der(nav_pos_y_integrated) = nav_vel_y_integrated;
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
  nav_vel_y_integrated = initial_vel_y;
  nav_vel_z = initial_vel_z;
end LanderNavigation;
