within LunCo.Sensors;

// A gravity-referenced attitude conversion using only IMU outputs.
//
// The estimator propagates the measured gravity direction with gyro rate and
// slowly corrects it toward the accelerometer direction.  It intentionally has
// no rigid-body quaternion input: truth attitude belongs to Avian, not to flight
// software.  The outputs are the body-frame upright error consumed by the
// attitude controller.
model AttitudeReference
  extends LunCo.Icons.Sensor;

  parameter Real correction_gain = 2.0 "Accelerometer correction rate (1/s)";
  parameter Real minimum_specific_force = 0.01 "Minimum force for gravity update (m/s2)";

  input Real specific_force_x = 0.0 "IMU specific force X (m/s2)";
  input Real specific_force_y = 0.0 "IMU specific force Y (m/s2)";
  input Real specific_force_z = 0.0 "IMU specific force Z (m/s2)";
  input Real gyro_x = 0.0 "IMU angular rate X (rad/s)";
  input Real gyro_y = 0.0 "IMU angular rate Y (rad/s)";
  input Real gyro_z = 0.0 "IMU angular rate Z (rad/s)";

  output Real error_x "Body-frame upright error about X";
  output Real error_y "Body-frame upright error about Y";
  output Real error_z "Body-frame upright error about Z";
  output Real estimated_up_x "Estimated world-up direction in body X";
  output Real estimated_up_y "Estimated world-up direction in body Y";
  output Real estimated_up_z "Estimated world-up direction in body Z";

  Real force_norm;
  Real measured_up_x;
  Real measured_up_y;
  Real measured_up_z;
  Real estimated_up_x_state(start = 0.0);
  Real estimated_up_y_state(start = 1.0);
  Real estimated_up_z_state(start = 0.0);

equation
  force_norm = sqrt(
    specific_force_x * specific_force_x
      + specific_force_y * specific_force_y
      + specific_force_z * specific_force_z);
  measured_up_x = if force_norm > minimum_specific_force then specific_force_x / force_norm
    else estimated_up_x_state;
  measured_up_y = if force_norm > minimum_specific_force then specific_force_y / force_norm
    else estimated_up_y_state;
  measured_up_z = if force_norm > minimum_specific_force then specific_force_z / force_norm
    else estimated_up_z_state;

  der(estimated_up_x_state) =
    -gyro_y * estimated_up_z_state + gyro_z * estimated_up_y_state
      + correction_gain * (measured_up_x - estimated_up_x_state);
  der(estimated_up_y_state) =
    -gyro_z * estimated_up_x_state + gyro_x * estimated_up_z_state
      + correction_gain * (measured_up_y - estimated_up_y_state);
  der(estimated_up_z_state) =
    -gyro_x * estimated_up_y_state + gyro_y * estimated_up_x_state
      + correction_gain * (measured_up_z - estimated_up_z_state);

  estimated_up_x = estimated_up_x_state;
  estimated_up_y = estimated_up_y_state;
  estimated_up_z = estimated_up_z_state;
  error_x = -estimated_up_z_state;
  error_y = 0.0;
  error_z = estimated_up_x_state;
end AttitudeReference;
