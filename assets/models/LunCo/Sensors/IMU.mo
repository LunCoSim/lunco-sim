within LunCo.Sensors;

// Inertial Measurement Unit (IMU) model: 3-axis accelerometer + 3-axis rate gyro.
model IMU
  parameter Real accel_bias_m_s2 = 0.02 "Accelerometer bias offset, m/s²";
  parameter Real gyro_bias_rad_s = 0.001 "Gyro rate bias offset, rad/s";

  input Real accel_x_true "True acceleration X, m/s²";
  input Real accel_y_true "True acceleration Y, m/s²";
  input Real accel_z_true "True acceleration Z, m/s²";

  output Real accel_x_meas "Measured acceleration X reported to GNC, m/s²";
  output Real accel_y_meas "Measured acceleration Y reported to GNC, m/s²";
  output Real accel_z_meas "Measured acceleration Z reported to GNC, m/s²";
equation
  accel_x_meas = accel_x_true + accel_bias_m_s2;
  accel_y_meas = accel_y_true + accel_bias_m_s2;
  accel_z_meas = accel_z_true + accel_bias_m_s2;
end IMU;
