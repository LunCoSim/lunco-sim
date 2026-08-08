within LunCo.Sensors;

// A reusable sensor conversion, not a physics provider.
//
// Avian supplies primitive solved rigid-body kinematics through ordinary ports:
// linear acceleration, angular velocity, and attitude. This model transports
// the solved acceleration to the mounted instrument frame and subtracts the
// authored local gravity vector. Bias and scale are intentionally parameters
// so each experiment can expose and tune them in USD.
model IMUSensor
  extends LunCo.Icons.Sensor;

  parameter Real mount_offset_x = 0.0 "Sensor offset from body COM, local X (m)";
  parameter Real mount_offset_y = 0.0 "Sensor offset from body COM, local Y (m)";
  parameter Real mount_offset_z = 0.0 "Sensor offset from body COM, local Z (m)";
  parameter Real accel_bias_x = 0.0 "Accelerometer X bias (m/s2)";
  parameter Real accel_bias_y = 0.0 "Accelerometer Y bias (m/s2)";
  parameter Real accel_bias_z = 0.0 "Accelerometer Z bias (m/s2)";
  parameter Real gyro_bias_x = 0.0 "Gyroscope X bias (rad/s)";
  parameter Real gyro_bias_y = 0.0 "Gyroscope Y bias (rad/s)";
  parameter Real gyro_bias_z = 0.0 "Gyroscope Z bias (rad/s)";
  parameter Real accel_scale_factor = 1.0 "Accelerometer scale factor";
  parameter Real quaternion_epsilon = 1e-12 "Quaternion normalization floor";
  parameter Real angular_accel_filter_time_constant_s = 0.02
    "Angular accelerometer differentiator time constant (s)";
  // Primitive Avian outputs.  These are deliberately named raw_* so a model
  // review can see that no semantic sensor value is entering the conversion.
  input Real raw_acceleration_x = 0.0 "Avian solved acceleration X (m/s2)";
  input Real raw_acceleration_y = 0.0 "Avian solved acceleration Y (m/s2)";
  input Real raw_acceleration_z = 0.0 "Avian solved acceleration Z (m/s2)";
  input Real raw_sample_valid = 0.0 "Avian acceleration sample validity";
  input Real raw_angvel_x = 0.0 "Avian AngularVelocity X (rad/s)";
  input Real raw_angvel_y = 0.0 "Avian AngularVelocity Y (rad/s)";
  input Real raw_angvel_z = 0.0 "Avian AngularVelocity Z (rad/s)";
  input Real raw_quat_w = 1.0 "Avian Rotation quaternion W";
  input Real raw_quat_x = 0.0 "Avian Rotation quaternion X";
  input Real raw_quat_y = 0.0 "Avian Rotation quaternion Y";
  input Real raw_quat_z = 0.0 "Avian Rotation quaternion Z";
  input Real gravity_world_x = 0.0 "Local gravity X in navigation/world frame (m/s2)";
  input Real gravity_world_y = 0.0 "Local gravity Y in navigation/world frame (m/s2)";
  input Real gravity_world_z = 0.0 "Local gravity Z in navigation/world frame (m/s2)";

  output Real coordinate_accel_local_x "Coordinate acceleration in sensor X (m/s2)";
  output Real coordinate_accel_local_y "Coordinate acceleration in sensor Y (m/s2)";
  output Real coordinate_accel_local_z "Coordinate acceleration in sensor Z (m/s2)";
  output Real specific_force_x "Specific force in sensor X (m/s2)";
  output Real specific_force_y "Specific force in sensor Y (m/s2)";
  output Real specific_force_z "Specific force in sensor Z (m/s2)";
  output Real gyro_x "Measured angular rate X (rad/s)";
  output Real gyro_y "Measured angular rate Y (rad/s)";
  output Real gyro_z "Measured angular rate Z (rad/s)";
  output Real sensor_health "1 when the raw Avian quaternion is usable";
  output Real attitude_quat_w "Measured attitude quaternion W";
  output Real attitude_quat_x "Measured attitude quaternion X";
  output Real attitude_quat_y "Measured attitude quaternion Y";
  output Real attitude_quat_z "Measured attitude quaternion Z";
  output Real attitude_quat_valid "1 when the measured attitude is usable";

  Real world_accel_x;
  Real world_accel_y;
  Real world_accel_z;
  Real angular_accel_x;
  Real angular_accel_y;
  Real angular_accel_z;
  Real offset_world_x;
  Real offset_world_y;
  Real offset_world_z;
  Real lever_accel_x;
  Real lever_accel_y;
  Real lever_accel_z;
  Real total_accel_x;
  Real total_accel_y;
  Real total_accel_z;
  Real sensor_accel_x;
  Real sensor_accel_y;
  Real sensor_accel_z;
  FrameVectorTransform offset_transform(
    quaternion_epsilon = quaternion_epsilon);
  FrameVectorTransform force_transform(
    quaternion_epsilon = quaternion_epsilon);
  FrameVectorTransform gravity_transform(
    quaternion_epsilon = quaternion_epsilon);
  FrameVectorTransform gyro_transform(
    quaternion_epsilon = quaternion_epsilon);
  FilteredDerivative angular_velocity_filter_x(
    time_constant_s = angular_accel_filter_time_constant_s);
  FilteredDerivative angular_velocity_filter_y(
    time_constant_s = angular_accel_filter_time_constant_s);
  FilteredDerivative angular_velocity_filter_z(
    time_constant_s = angular_accel_filter_time_constant_s);

equation

  // Avian's world-frame acceleration is sampled from the solved rigid body.
  // It is already the physical accelerometer primitive: gravity, thrust,
  // contacts, and joints are in the solved state. Do not differentiate or
  // delay a co-simulation signal here; that would manufacture a startup
  // transient when the Modelica instance is admitted after the first sample.
  angular_velocity_filter_x.u = raw_angvel_x;
  angular_velocity_filter_y.u = raw_angvel_y;
  angular_velocity_filter_z.u = raw_angvel_z;
  angular_velocity_filter_x.sample_valid = raw_sample_valid;
  angular_velocity_filter_y.sample_valid = raw_sample_valid;
  angular_velocity_filter_z.sample_valid = raw_sample_valid;
  world_accel_x = raw_acceleration_x;
  world_accel_y = raw_acceleration_y;
  world_accel_z = raw_acceleration_z;
  angular_accel_x = angular_velocity_filter_x.y;
  angular_accel_y = angular_velocity_filter_y.y;
  angular_accel_z = angular_velocity_filter_z.y;

  offset_transform.quaternion_w = raw_quat_w;
  offset_transform.quaternion_x = raw_quat_x;
  offset_transform.quaternion_y = raw_quat_y;
  offset_transform.quaternion_z = raw_quat_z;
  offset_transform.vector_x = mount_offset_x;
  offset_transform.vector_y = mount_offset_y;
  offset_transform.vector_z = mount_offset_z;
  offset_world_x = offset_transform.world_frame_x;
  offset_world_y = offset_transform.world_frame_y;
  offset_world_z = offset_transform.world_frame_z;

  // Rigid-body transport: alpha x r + omega x (omega x r).
  lever_accel_x =
    angular_accel_y * offset_world_z - angular_accel_z * offset_world_y
      + raw_angvel_y * (raw_angvel_x * offset_world_y - raw_angvel_y * offset_world_x)
      - raw_angvel_z * (raw_angvel_z * offset_world_x - raw_angvel_x * offset_world_z);
  lever_accel_y =
    angular_accel_z * offset_world_x - angular_accel_x * offset_world_z
      + raw_angvel_z * (raw_angvel_y * offset_world_z - raw_angvel_z * offset_world_y)
      - raw_angvel_x * (raw_angvel_x * offset_world_y - raw_angvel_y * offset_world_x);
  lever_accel_z =
    angular_accel_x * offset_world_y - angular_accel_y * offset_world_x
      + raw_angvel_x * (raw_angvel_z * offset_world_x - raw_angvel_x * offset_world_z)
      - raw_angvel_y * (raw_angvel_y * offset_world_z - raw_angvel_z * offset_world_y);

  total_accel_x = world_accel_x + lever_accel_x;
  total_accel_y = world_accel_y + lever_accel_y;
  total_accel_z = world_accel_z + lever_accel_z;

  // Avian linear acceleration and the environment gravity source are both
  // navigation/world-frame vectors. Convert each through the same shared
  // transform before subtracting them. This is the specific-force equation
  // used by a real accelerometer: f_body = a_body - g_body.
  force_transform.quaternion_w = raw_quat_w;
  force_transform.quaternion_x = raw_quat_x;
  force_transform.quaternion_y = raw_quat_y;
  force_transform.quaternion_z = raw_quat_z;
  force_transform.vector_x = total_accel_x;
  force_transform.vector_y = total_accel_y;
  force_transform.vector_z = total_accel_z;
  gravity_transform.quaternion_w = raw_quat_w;
  gravity_transform.quaternion_x = raw_quat_x;
  gravity_transform.quaternion_y = raw_quat_y;
  gravity_transform.quaternion_z = raw_quat_z;
  gravity_transform.vector_x = gravity_world_x;
  gravity_transform.vector_y = gravity_world_y;
  gravity_transform.vector_z = gravity_world_z;
  sensor_accel_x = force_transform.body_frame_x - gravity_transform.body_frame_x;
  sensor_accel_y = force_transform.body_frame_y - gravity_transform.body_frame_y;
  sensor_accel_z = force_transform.body_frame_z - gravity_transform.body_frame_z;

  coordinate_accel_local_x = sensor_accel_x * accel_scale_factor + accel_bias_x;
  coordinate_accel_local_y = sensor_accel_y * accel_scale_factor + accel_bias_y;
  coordinate_accel_local_z = sensor_accel_z * accel_scale_factor + accel_bias_z;
  specific_force_x = coordinate_accel_local_x;
  specific_force_y = coordinate_accel_local_y;
  specific_force_z = coordinate_accel_local_z;

  // The same shared conversion produces body-frame gyroscope rates.
  gyro_transform.quaternion_w = raw_quat_w;
  gyro_transform.quaternion_x = raw_quat_x;
  gyro_transform.quaternion_y = raw_quat_y;
  gyro_transform.quaternion_z = raw_quat_z;
  gyro_transform.vector_x = raw_angvel_x;
  gyro_transform.vector_y = raw_angvel_y;
  gyro_transform.vector_z = raw_angvel_z;
  gyro_x = gyro_transform.body_frame_x + gyro_bias_x;
  gyro_y = gyro_transform.body_frame_y + gyro_bias_y;
  gyro_z = gyro_transform.body_frame_z + gyro_bias_z;
  attitude_quat_w = force_transform.normalized_quaternion_w;
  attitude_quat_x = force_transform.normalized_quaternion_x;
  attitude_quat_y = force_transform.normalized_quaternion_y;
  attitude_quat_z = force_transform.normalized_quaternion_z;
  sensor_health = force_transform.quaternion_valid;
  attitude_quat_valid = force_transform.quaternion_valid;
end IMUSensor;
