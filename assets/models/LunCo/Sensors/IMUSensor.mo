within LunCo.Sensors;

// A reusable sensor conversion, not a physics provider.
//
// Avian supplies the primitive rigid-body state through ordinary ports:
// velocity, angular velocity, and attitude.  This model differentiates the
// velocity state, transports the result to the mounted instrument frame, and
// subtracts the authored local gravity vector.  Bias and scale are intentionally
// parameters so each experiment can expose and tune them in USD.
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
  parameter Real accel_filter_time_constant_s = 0.02
    "Accelerometer differentiator time constant (s)";
  parameter Real angular_accel_filter_time_constant_s = 0.02
    "Angular accelerometer differentiator time constant (s)";

  // Primitive Avian outputs.  These are deliberately named raw_* so a model
  // review can see that no semantic sensor value is entering the conversion.
  input Real raw_velocity_x = 0.0 "Avian LinearVelocity X (m/s)";
  input Real raw_velocity_y = 0.0 "Avian LinearVelocity Y (m/s)";
  input Real raw_velocity_z = 0.0 "Avian LinearVelocity Z (m/s)";
  input Real raw_angvel_x = 0.0 "Avian AngularVelocity X (rad/s)";
  input Real raw_angvel_y = 0.0 "Avian AngularVelocity Y (rad/s)";
  input Real raw_angvel_z = 0.0 "Avian AngularVelocity Z (rad/s)";
  input Real raw_quat_w = 1.0 "Avian Rotation quaternion W";
  input Real raw_quat_x = 0.0 "Avian Rotation quaternion X";
  input Real raw_quat_y = 0.0 "Avian Rotation quaternion Y";
  input Real raw_quat_z = 0.0 "Avian Rotation quaternion Z";
  input Real gravity_x = 0.0 "Local gravity X in the Avian frame (m/s2)";
  input Real gravity_y = 0.0 "Local gravity Y in the Avian frame (m/s2)";
  input Real gravity_z = 0.0 "Local gravity Z in the Avian frame (m/s2)";

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

  Real q_norm_sq;
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
  Real force_x;
  Real force_y;
  Real force_z;
  Real sensor_accel_x;
  Real sensor_accel_y;
  Real sensor_accel_z;
  FilteredDerivative velocity_filter_x(
    time_constant_s = accel_filter_time_constant_s);
  FilteredDerivative velocity_filter_y(
    time_constant_s = accel_filter_time_constant_s);
  FilteredDerivative velocity_filter_z(
    time_constant_s = accel_filter_time_constant_s);
  FilteredDerivative angular_velocity_filter_x(
    time_constant_s = angular_accel_filter_time_constant_s);
  FilteredDerivative angular_velocity_filter_y(
    time_constant_s = angular_accel_filter_time_constant_s);
  FilteredDerivative angular_velocity_filter_z(
    time_constant_s = angular_accel_filter_time_constant_s);

equation
  q_norm_sq = max(quaternion_epsilon,
    raw_quat_w * raw_quat_w + raw_quat_x * raw_quat_x
      + raw_quat_y * raw_quat_y + raw_quat_z * raw_quat_z);

  // Avian's world-frame velocity is a primitive state. A bounded-bandwidth
  // differentiator keeps the cross-engine boundary structurally valid while
  // modelling the finite response of a real accelerometer.
  velocity_filter_x.u = raw_velocity_x;
  velocity_filter_y.u = raw_velocity_y;
  velocity_filter_z.u = raw_velocity_z;
  angular_velocity_filter_x.u = raw_angvel_x;
  angular_velocity_filter_y.u = raw_angvel_y;
  angular_velocity_filter_z.u = raw_angvel_z;
  world_accel_x = velocity_filter_x.y;
  world_accel_y = velocity_filter_y.y;
  world_accel_z = velocity_filter_z.y;
  angular_accel_x = angular_velocity_filter_x.y;
  angular_accel_y = angular_velocity_filter_y.y;
  angular_accel_z = angular_velocity_filter_z.y;

  // Rotate the authored COM offset into the Avian frame.
  offset_world_x =
    ((1.0 - 2.0 * (raw_quat_y * raw_quat_y + raw_quat_z * raw_quat_z)) * mount_offset_x
      + 2.0 * (raw_quat_x * raw_quat_y - raw_quat_w * raw_quat_z) * mount_offset_y
      + 2.0 * (raw_quat_x * raw_quat_z + raw_quat_w * raw_quat_y) * mount_offset_z);
  offset_world_y =
    (2.0 * (raw_quat_x * raw_quat_y + raw_quat_w * raw_quat_z) * mount_offset_x
      + (1.0 - 2.0 * (raw_quat_x * raw_quat_x + raw_quat_z * raw_quat_z)) * mount_offset_y
      + 2.0 * (raw_quat_y * raw_quat_z - raw_quat_w * raw_quat_x) * mount_offset_z);
  offset_world_z =
    (2.0 * (raw_quat_x * raw_quat_z - raw_quat_w * raw_quat_y) * mount_offset_x
      + 2.0 * (raw_quat_y * raw_quat_z + raw_quat_w * raw_quat_x) * mount_offset_y
      + (1.0 - 2.0 * (raw_quat_x * raw_quat_x + raw_quat_y * raw_quat_y)) * mount_offset_z);

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

  // Convert Avian-frame acceleration and gravity into the instrument frame.
  force_x = total_accel_x - gravity_x;
  force_y = total_accel_y - gravity_y;
  force_z = total_accel_z - gravity_z;
  sensor_accel_x =
    (1.0 - 2.0 * (raw_quat_y * raw_quat_y + raw_quat_z * raw_quat_z)) * force_x
      + 2.0 * (raw_quat_x * raw_quat_y + raw_quat_w * raw_quat_z) * force_y
      + 2.0 * (raw_quat_x * raw_quat_z - raw_quat_w * raw_quat_y) * force_z;
  sensor_accel_y =
    2.0 * (raw_quat_x * raw_quat_y - raw_quat_w * raw_quat_z) * force_x
      + (1.0 - 2.0 * (raw_quat_x * raw_quat_x + raw_quat_z * raw_quat_z)) * force_y
      + 2.0 * (raw_quat_y * raw_quat_z + raw_quat_w * raw_quat_x) * force_z;
  sensor_accel_z =
    2.0 * (raw_quat_x * raw_quat_z + raw_quat_w * raw_quat_y) * force_x
      + 2.0 * (raw_quat_y * raw_quat_z - raw_quat_w * raw_quat_x) * force_y
      + (1.0 - 2.0 * (raw_quat_x * raw_quat_x + raw_quat_y * raw_quat_y)) * force_z;

  coordinate_accel_local_x = sensor_accel_x * accel_scale_factor + accel_bias_x;
  coordinate_accel_local_y = sensor_accel_y * accel_scale_factor + accel_bias_y;
  coordinate_accel_local_z = sensor_accel_z * accel_scale_factor + accel_bias_z;
  specific_force_x = coordinate_accel_local_x;
  specific_force_y = coordinate_accel_local_y;
  specific_force_z = coordinate_accel_local_z;

  gyro_x = raw_angvel_x + gyro_bias_x;
  gyro_y = raw_angvel_y + gyro_bias_y;
  gyro_z = raw_angvel_z + gyro_bias_z;
  sensor_health = if raw_quat_w * raw_quat_w + raw_quat_x * raw_quat_x
      + raw_quat_y * raw_quat_y + raw_quat_z * raw_quat_z
      > quaternion_epsilon then 1.0 else 0.0;
end IMUSensor;
