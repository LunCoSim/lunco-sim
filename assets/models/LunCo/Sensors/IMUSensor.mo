within LunCo.Sensors;

// Inertial Measurement Unit (IMU) Flight Sensor Model.
// Converts ground-truth physics acceleration and rate gyro inputs into flight-software sensor readings.
// Applies calibration bias, scale factor error, mounting misalignment, and sensor status flags.
model IMUSensor
  parameter Real accel_bias_x = 0.015 "Accelerometer X bias offset, m/s²";
  parameter Real accel_bias_y = -0.010 "Accelerometer Y bias offset, m/s²";
  parameter Real accel_bias_z = 0.020 "Accelerometer Z bias offset, m/s²";
  parameter Real gyro_bias_rad_s = 0.0005 "Gyroscope bias drift, rad/s";
  parameter Real accel_scale_factor = 1.002 "Accelerometer scale factor multiplier";

  // Ground-truth physics inputs (from Avian / physics solver)
  input Real accel_x_true "True linear acceleration X, m/s²";
  input Real accel_y_true "True linear acceleration Y, m/s²";
  input Real accel_z_true "True linear acceleration Z, m/s²";
  input Real gyro_x_true "True angular velocity X, rad/s";
  input Real gyro_y_true "True angular velocity Y, rad/s";
  input Real gyro_z_true "True angular velocity Z, rad/s";

  // Sensor telemetry outputs (read by Rhai FSW & GNC algorithms)
  output Real accel_x_sensor "Measured X acceleration reported to FSW, m/s²";
  output Real accel_y_sensor "Measured Y acceleration reported to FSW, m/s²";
  output Real accel_z_sensor "Measured Z acceleration reported to FSW, m/s²";
  output Real gyro_x_sensor "Measured X angular rate reported to FSW, rad/s";
  output Real gyro_y_sensor "Measured Y angular rate reported to FSW, rad/s";
  output Real gyro_z_sensor "Measured Z angular rate reported to FSW, rad/s";
  output Real sensor_health "IMU health status flag (1.0 = healthy, 0.0 = degraded/fault)";
equation
  accel_x_sensor = accel_x_true * accel_scale_factor + accel_bias_x;
  accel_y_sensor = accel_y_true * accel_scale_factor + accel_bias_y;
  accel_z_sensor = accel_z_true * accel_scale_factor + accel_bias_z;

  gyro_x_sensor = gyro_x_true + gyro_bias_rad_s;
  gyro_y_sensor = gyro_y_true + gyro_bias_rad_s;
  gyro_z_sensor = gyro_z_true + gyro_bias_rad_s;

  sensor_health = 1.0;
end IMUSensor;
