within LunCo.Sensors;

// Star Tracker Optical Attitude Sensor Model for Spacecraft & Lander GNC.
// Determines vehicle orientation quaternion from star field imagery.
// Models sun/earth blinding exclusion angles, high-angular-rate tracking loss, and lock status flags.
model StarTracker
  parameter Real sun_exclusion_angle_deg = 30.0 "Sun-in-field-of-view exclusion mask angle, deg";
  parameter Real max_rate_deg_s = 2.5 "Maximum angular rate tracking limit, deg/s";

  input Real q_w_true "True attitude quaternion W";
  input Real q_x_true "True attitude quaternion X";
  input Real q_y_true "True attitude quaternion Y";
  input Real q_z_true "True attitude quaternion Z";

  input Real sun_angle_deg "Angle between star tracker boresight and Sun vector, deg";
  input Real body_rate_deg_s "Current vehicle body angular rate magnitude, deg/s";

  output Real q_w_sensor "Reported attitude quaternion W";
  output Real q_x_sensor "Reported attitude quaternion X";
  output Real q_y_sensor "Reported attitude quaternion Y";
  output Real q_z_sensor "Reported attitude quaternion Z";
  output Real tracking_lock "Attitude lock status (1.0 = valid attitude lock, 0.0 = blinded/lost lock)";
equation
  tracking_lock = min(
    max(0.0, min(1.0, 0.5 + sun_angle_deg - sun_exclusion_angle_deg)),
    max(0.0, min(1.0, 0.5 + max_rate_deg_s - body_rate_deg_s)));
  q_w_sensor = 1.0 + tracking_lock * (q_w_true - 1.0);
  q_x_sensor = tracking_lock * q_x_true;
  q_y_sensor = tracking_lock * q_y_true;
  q_z_sensor = tracking_lock * q_z_true;
end StarTracker;
