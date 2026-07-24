within LunCo.GNC;

// Lander Attitude Rate & Position PID Feedback Controller Model.
// Calculates continuous control torques and forces in Modelica (zero math in Rhai).
model LanderPID
  parameter Real kp_pitch = 4.5 "Proportional gain for pitch control";
  parameter Real kd_pitch = 2.1 "Derivative gain for pitch rate dampening";
  parameter Real kp_z = 3.0 "Proportional gain for altitude descent control";
  parameter Real kd_z = 1.5 "Derivative gain for vertical velocity dampening";

  input Real pos_z "Current altitude Z, m";
  input Real vel_z "Current vertical velocity Z, m/s";
  input Real target_pos_z "Commanded target altitude Z, m";
  input Real pitch_deg "Current lander pitch attitude, deg";
  input Real pitch_rate_deg_s "Current lander pitch rate, deg/s";
  input Real target_pitch_deg "Commanded target pitch attitude, deg";

  output Real f_cmd_z "Commanded vertical thrust force, N";
  output Real tau_cmd_pitch "Commanded pitch control torque, N.m";

  Real err_z "Altitude error, m";
  Real err_pitch "Pitch attitude error, deg";
equation
  err_z = target_pos_z - pos_z;
  err_pitch = target_pitch_deg - pitch_deg;

  f_cmd_z = max(0.0, kp_z * err_z - kd_z * vel_z);
  tau_cmd_pitch = kp_pitch * err_pitch - kd_pitch * pitch_rate_deg_s;
end LanderPID;
