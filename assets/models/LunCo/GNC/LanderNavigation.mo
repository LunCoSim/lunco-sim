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
  input Real altimeter_position_valid = 0.0
    "1 when the ray hit provides a valid vehicle X/Z observation";
  input Real altimeter_altitude_confidence = 0.0
    "0..1 confidence that the ray provides vertical altitude evidence";
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
  input Real gravity_nav_x = 0.0 "Gravity in the navigation X axis (m/s²)";
  input Real gravity_nav_y = -1.62 "Gravity in the navigation Y axis (m/s²)";
  input Real gravity_nav_z = 0.0 "Gravity in the navigation Z axis (m/s²)";
  parameter Real quaternion_epsilon = 1.0e-12 "Quaternion normalization floor";
  parameter Real altitude_position_correction_gain = 4.0
    "Complementary altitude position correction gain (1/s)";
  parameter Real altitude_observation_acquisition_time_constant_s = 0.5
    "Continuous authority ramp when vertical altitude evidence returns (s)";
  input Real altitude_velocity_correction_gain = 4.0
    "Complementary altitude velocity correction gain (1/s2)";
  input Real lateral_position_correction_gain = 2.0
    "Complementary terrain-hit lateral position gain (1/s)";
  input Real lateral_velocity_correction_gain = 1.0
    "Complementary terrain-hit lateral velocity gain (1/s2)";

  parameter Real initial_pos_x = 0.0 "Mission-initialized X position (m)";
  parameter Real initial_pos_y = 0.0 "Mission-initialized Y position (m)";
  parameter Real initial_pos_z = 0.0 "Mission-initialized Z position (m)";
  parameter Real initial_vel_x = 0.0 "Mission-initialized X velocity (m/s)";
  parameter Real initial_vel_y = 0.0 "Mission-initialized Y velocity (m/s)";
  parameter Real initial_vel_z = 0.0 "Mission-initialized Z velocity (m/s)";
  output Real nav_pos_x(unit = "m") "Estimated X position";
  output Real nav_pos_y(unit = "m") "Estimated Y position from altimeter";
  output Real nav_pos_z(unit = "m") "Estimated Z position";
  output Real nav_vel_x(unit = "m/s") "Estimated X velocity";
  output Real nav_vel_y(unit = "m/s") "Estimated Y velocity";
  output Real nav_vel_z(unit = "m/s") "Estimated Z velocity";
  output Real measured_altitude(unit = "m") "Raw altimeter measurement";

  Real measured_altitude_value "Conditioned altitude telemetry";
  Real navigation_accel_x;
  Real navigation_accel_y;
  Real navigation_accel_z;
  Real lateral_position_error_x;
  Real lateral_position_error_z;
  Real position_observation_valid;
  Real altitude_observation_confidence;
  Real altitude_position_error
    "Geometric altitude innovation used by the observer";
  LunCo.Sensors.FilteredSignal altitude_observation_authority(
    time_constant_s = 0.02,
    acquisition_time_constant_s = altitude_observation_acquisition_time_constant_s);
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

  // Integrate navigation-frame acceleration for velocity and lateral position.
  // Vertical position and velocity form the same continuous observer as the
  // lateral axes: IMU acceleration propagates the state, while a valid
  // geometric altimeter height corrects both position and velocity drift.
  // The altimeter's slant-range derivative is not treated as world-Y speed:
  // during attitude recovery it is the derivative of an oblique ray.
  // A nadir terrain return also carries the horizontal location of the ray
  // hit. Use it as a complementary position correction so IMU integration
  // remains the high-rate propagation while long-flight lateral drift stays
  // observable. During attitude recovery the ray is invalid and this term
  // naturally disappears.
  lateral_position_error_x = altimeter_vehicle_position_x - nav_pos_x;
  lateral_position_error_z = altimeter_vehicle_position_z - nav_pos_z;
  // Alpha-beta observer correction: the terrain hit corrects both position
  // drift and the velocity integral that would otherwise acquire an incorrect
  // sign during a long powered descent.  This is the ordinary continuous
  // position/velocity observer, not a truth-position feed or a scripted path.
  der(nav_pos_x) = nav_vel_x + lateral_position_correction_gain
    * position_observation_valid * lateral_position_error_x;
  der(nav_pos_z) = nav_vel_z + lateral_position_correction_gain
    * position_observation_valid * lateral_position_error_z;
  der(nav_vel_x) = navigation_accel_x + lateral_velocity_correction_gain
    * position_observation_valid * lateral_position_error_x;
  position_observation_valid = max(0.0, min(1.0, altimeter_position_valid));
  altitude_observation_confidence = max(0.0,
    min(1.0, altimeter_altitude_confidence));
  // A nadir return can disappear and reappear as the airframe rotates.  The
  // first valid sample after that gap may have a large geometric innovation
  // because the IMU-only state has continued to propagate.  Reuse the
  // canonical sensor acquisition primitive so measurement authority itself is
  // a continuous state: it acquires the evidence with its authored time
  // constant and begins releasing it as soon as the evidence is gone.  This is
  // sensor dynamics, not a frame-count delay or a mission-script handoff.
  altitude_observation_authority.u = 0.0;
  altitude_observation_authority.sample_valid = altitude_observation_confidence;
  altitude_position_error = altimeter_vehicle_position_y - nav_pos_y;
  der(nav_pos_y) = nav_vel_y + altitude_position_correction_gain
    * altitude_observation_authority.acquisition * altitude_position_error;
  der(nav_vel_y) = navigation_accel_y + altitude_velocity_correction_gain
    * altitude_observation_authority.acquisition * altitude_position_error;
  der(nav_vel_z) = navigation_accel_z + lateral_velocity_correction_gain
    * position_observation_valid * lateral_position_error_z;

  measured_altitude_value = altimeter_range;
  measured_altitude = measured_altitude_value;

initial equation
  nav_pos_x = initial_pos_x;
  nav_pos_y = initial_pos_y;
  nav_pos_z = initial_pos_z;
  nav_vel_x = initial_vel_x;
  nav_vel_y = initial_vel_y;
  nav_vel_z = initial_vel_z;
end LanderNavigation;
