within LunCo.GNC;

// Three-axis position PID guidance for a powered lander.
//
// The three PIDAxis instances form the visible Modelica scheme:
//   target position/rate - measured position/rate -> PID X/Y/Z
//   PID acceleration vector -> thrust magnitude + commanded tilt
//
// This model owns the navigation and control mathematics.  USD owns the target
// body, sensor/actuator wiring, and live gain values; Rhai owns only mission
// sequencing.  There is no per-tick script controller and no Modelica Standard
// Library dependency.
model PositionPID3D
  extends LunCo.Icons.Guidance;

  // Vehicle state and mission target, all in the scene's world frame (+Y up).
  input Real pos_x = 0.0 "Vehicle position X (m)";
  input Real pos_y = 0.0 "Vehicle position Y (m)";
  input Real pos_z = 0.0 "Vehicle position Z (m)";
  input Real vel_x = 0.0 "Vehicle velocity X (m/s)";
  input Real vel_y = 0.0 "Vehicle velocity Y (m/s)";
  input Real vel_z = 0.0 "Vehicle velocity Z (m/s)";

  input Real target_x = 0.0 "Landing target X (m)";
  input Real target_y = 5.0 "Landing target Y (m)";
  input Real target_z = 0.0 "Landing target Z (m)";
  input Real target_vel_x = 0.0 "Landing target velocity X (m/s)";
  input Real target_vel_y = 0.0 "Landing target velocity Y (m/s)";
  input Real target_vel_z = 0.0 "Landing target velocity Z (m/s)";

  // Live tuning inputs.  These are inputs, not parameters, so USD Inspector
  // edits change both comparison vehicles without rebuilding the scene.
  input Real kp_x = 0.08 "X proportional gain (1/s²)";
  input Real ki_x = 0.003 "X integral gain (1/s³)";
  input Real kd_x = 0.75 "X derivative gain (1/s)";
  input Real kp_y = 0.18 "Y proportional gain (1/s²)";
  input Real ki_y = 0.006 "Y integral gain (1/s³)";
  input Real kd_y = 1.20 "Y derivative gain (1/s)";
  input Real kp_z = 0.08 "Z proportional gain (1/s²)";
  input Real ki_z = 0.003 "Z integral gain (1/s³)";
  input Real kd_z = 0.75 "Z derivative gain (1/s)";
  input Real integral_limit = 12.0 "Integral state limit";
  input Real anti_windup_gain = 1.0 "PID anti-windup back-calculation";
  input Real max_lateral_accel = 4.0 "Maximum lateral acceleration (m/s²)";
  input Real max_vertical_accel = 8.0 "Maximum vertical correction (m/s²)";
  input Real g = 1.62 "Local gravity (m/s²)";
  input Real max_thrust = 60000.0 "Maximum engine thrust (N)";
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg)";

  // The airframe uses normalized guidance commands.
  input Real piloted = 0.0 "1 while a pilot owns the vehicle";
  input Real engage = 1.0 "1 while this mission guidance is active";
  output Real throttle_cmd "Main-engine command, 0..1";
  output Real pitch_cmd "Lateral tilt command, -1..1";
  output Real roll_cmd "Lateral tilt command, -1..1";
  output Real yaw_cmd "Heading command, -1..1";

  // Evidence channels for the teaching scene.
  output Real target_distance_m(unit = "m") "Distance to the landing target";
  output Real lateral_accel_x(unit = "m/s2") "PID X acceleration command";
  output Real vertical_accel(unit = "m/s2") "Gravity-compensated Y acceleration";
  output Real lateral_accel_z(unit = "m/s2") "PID Z acceleration command";
  output Real position_error_x(unit = "m") "X position error";
  output Real position_error_y(unit = "m") "Y position error";
  output Real position_error_z(unit = "m") "Z position error";
  output Real pid_x_command(unit = "m/s2") "Saturated X PID output";
  output Real pid_y_command(unit = "m/s2") "Saturated Y PID output";
  output Real pid_z_command(unit = "m/s2") "Saturated Z PID output";

  PIDAxis pid_x;
  PIDAxis pid_y;
  PIDAxis pid_z;

  Real max_thrust_accel;
  Real thrust_accel;
  Real unsaturated_throttle;

equation
  // The component assignments are the readable block-diagram wiring.  Each
  // axis receives setpoint, feedback, and its own live gains.
  pid_x.setpoint = target_x;
  pid_x.measurement = pos_x;
  pid_x.setpoint_rate = target_vel_x;
  pid_x.measurement_rate = vel_x;
  pid_x.kp = kp_x;
  pid_x.ki = ki_x;
  pid_x.kd = kd_x;
  pid_x.integral_limit = integral_limit;
  pid_x.output_limit = max_lateral_accel;
  pid_x.anti_windup_gain = anti_windup_gain;

  pid_y.setpoint = target_y;
  pid_y.measurement = pos_y;
  pid_y.setpoint_rate = target_vel_y;
  pid_y.measurement_rate = vel_y;
  pid_y.kp = kp_y;
  pid_y.ki = ki_y;
  pid_y.kd = kd_y;
  pid_y.integral_limit = integral_limit;
  pid_y.output_limit = max_vertical_accel;
  pid_y.anti_windup_gain = anti_windup_gain;

  pid_z.setpoint = target_z;
  pid_z.measurement = pos_z;
  pid_z.setpoint_rate = target_vel_z;
  pid_z.measurement_rate = vel_z;
  pid_z.kp = kp_z;
  pid_z.ki = ki_z;
  pid_z.kd = kd_z;
  pid_z.integral_limit = integral_limit;
  pid_z.output_limit = max_lateral_accel;
  pid_z.anti_windup_gain = anti_windup_gain;

  max_thrust_accel = max_thrust / max(1.0, vehicle_mass);
  lateral_accel_x = pid_x.command;
  lateral_accel_z = pid_z.command;
  vertical_accel = max(0.0, min(max_vertical_accel, g + pid_y.command));
  thrust_accel = sqrt(lateral_accel_x * lateral_accel_x
    + vertical_accel * vertical_accel
    + lateral_accel_z * lateral_accel_z);
  unsaturated_throttle = thrust_accel / max(1.0, max_thrust_accel);

  // With body +Y as the engine axis, lateral acceleration is represented as a
  // bounded tilt request.  The airframe's own attitude stabilizer closes the
  // angular loop and turns these normalized requests into RCS torque.
  throttle_cmd = engage * (1.0 - piloted)
    * max(0.0, min(1.0, unsaturated_throttle));
  pitch_cmd = engage * (1.0 - piloted)
    * max(-1.0, min(1.0, -lateral_accel_z / max(1.0, vertical_accel)));
  roll_cmd = engage * (1.0 - piloted)
    * max(-1.0, min(1.0, lateral_accel_x / max(1.0, vertical_accel)));
  yaw_cmd = 0.0;

  target_distance_m = sqrt((target_x - pos_x) * (target_x - pos_x)
    + (target_y - pos_y) * (target_y - pos_y)
    + (target_z - pos_z) * (target_z - pos_z));
  position_error_x = pid_x.error;
  position_error_y = pid_y.error;
  position_error_z = pid_z.error;
  pid_x_command = pid_x.command;
  pid_y_command = pid_y.command;
  pid_z_command = pid_z.command;
end PositionPID3D;
