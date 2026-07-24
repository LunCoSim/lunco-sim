within LunCo.Sensors;

// Rotary Encoder Angle Sensor Model.
// Measures mechanical joint angle and angular velocity, modeling encoder pulses per revolution (PPR)
// quantization and digital angle telemetry for servo position feedback loops.
model EncoderSensor
  parameter Real ppr = 4096.0 "Encoder resolution, pulses per revolution (PPR)";
  parameter Real angle_bias_deg = 0.05 "Zero-point calibration offset, deg";

  input Real angle_true_rad "True mechanical joint angle, rad";
  input Real speed_true_rad_s "True mechanical joint angular velocity, rad/s";

  output Real angle_sensor_deg "Reported joint angle telemetry, deg";
  output Real angle_sensor_rad "Reported joint angle telemetry, rad";
  output Real speed_sensor_rad_s "Reported joint angular rate telemetry, rad/s";
  output Real encoder_counts "Encoder digital pulse count, 0..PPR";
equation
  angle_sensor_rad = angle_true_rad + (angle_bias_deg * 0.0174533);
  angle_sensor_deg = angle_sensor_rad * 57.2958;
  speed_sensor_rad_s = speed_true_rad_s;
  encoder_counts = floor(mod(angle_sensor_rad / (2.0 * 3.14159265), 1.0) * ppr);
end EncoderSensor;
