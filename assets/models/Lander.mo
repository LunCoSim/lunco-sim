// tagline: Lander GNC — 3D speed-limited position + quaternion attitude PD control + manual override
model Lander
  "Powered-descent guidance + 3D position and rotation tracking controller.
   Translates Lander to track LandingLocation in 3D with speed limits,
   and orients Lander to match target rotation. Gravity is read from
   the environment."

  // ── Vehicle parameters ──
  parameter Real vehicle_mass = 2000.0 "Vehicle mass (kg) — keep == USD physics:mass";
  parameter Real max_thrust = 60000.0 "Max engine thrust (N)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s)";
  
  // ── Gain parameters ──
  parameter Real kp_pos = 4.0 "Position tracking gain";
  parameter Real kp_rot = 15.0 "Rotation proportional gain";
  parameter Real kd_rot = 8.0 "Rotation derivative gain";

  // ── Custom speeds ──
  input Real speed = 5.0 "Maximum translation speed (m/s)";
  input Real angular_speed = 2.0 "Maximum rotation speed (rad/s)";

  // ── Target location inputs (from LandingLocation) ──
  input Real target_x = 0.0 "Target X position";
  input Real target_y = 0.0 "Target Y position (ground level)";
  input Real target_z = 0.0 "Target Z position";
  input Real target_qw = 1.0 "Target orientation quat w";
  input Real target_qx = 0.0 "Target orientation quat x";
  input Real target_qy = 0.0 "Target orientation quat y";
  input Real target_qz = 0.0 "Target orientation quat z";

  // ── Current state inputs (from Avian body) ──
  input Real pos_x = 0.0 "Current world X position";
  input Real pos_y = 60.0 "Current world Y position";
  input Real pos_z = 0.0 "Current world Z position";
  input Real altitude = 60.0 "Measured altitude from altimeter (m)";
  input Real vel_x = 0.0 "Current world X velocity";
  input Real vel_y = 0.0 "Current world Y velocity";
  input Real vel_z = 0.0 "Current world Z velocity";
  input Real q_w = 1.0 "Current orientation quat w";
  input Real q_x = 0.0 "Current orientation quat x";
  input Real q_y = 0.0 "Current orientation quat y";
  input Real q_z = 0.0 "Current orientation quat z";
  input Real w_x = 0.0 "Current angular velocity X";
  input Real w_y = 0.0 "Current angular velocity Y";
  input Real w_z = 0.0 "Current angular velocity Z";
  
  input Real g = 1.62 "Local gravity (m/s^2)";
  input Real engine_enable = 1.0 "1 = engine armed, 0 = cut";

  // ── Manual Override Inputs ──
  input Real manual = 0.0 "1 = manual control active, 0 = GNC active";
  input Real manual_throttle = 0.0 "Player vertical thrust command 0..1";
  input Real manual_pitch = 0.0 "Player pitch rate command -1..1";
  input Real manual_roll = 0.0 "Player roll rate command -1..1";
  input Real manual_yaw = 0.0 "Player yaw rate command -1..1";

  // ── Propellant State ──
  Real m_prop(start = 2000.0) "Propellant remaining (kg)";
  Real thrust "Total magnitude of thrust force";

  // ── Outputs (wired to Avian world forces & torques) ──
  output Real force_x "Commanded world X force (N)";
  output Real force_y "Commanded world Y force (N)";
  output Real force_z "Commanded world Z force (N)";
  output Real torque_x "Commanded world X torque (N*m)";
  output Real torque_y "Commanded world Y torque (N*m)";
  output Real torque_z "Commanded world Z torque (N*m)";
  output Real throttle "Effective throttle fraction 0..1";

  // ── Manual Control State (first-order spool lag) ──
  Real filter_throttle(start = 0.0);
  Real filter_pitch(start = 0.0);
  Real filter_roll(start = 0.0);
  Real filter_yaw(start = 0.0);

  // Local force and torque vectors
  Real f_loc_x, f_loc_y, f_loc_z;
  Real t_loc_x, t_loc_y, t_loc_z;

  // Rotated world vectors
  Real f_world_x, f_world_y, f_world_z;
  Real t_world_x, t_world_y, t_world_z;

  // GNC (autonomous) world outputs
  Real gnc_force_x, gnc_force_y, gnc_force_z;
  Real gnc_torque_x, gnc_torque_y, gnc_torque_z;

  // ── Intermediate variables ──
  Real dx, dy, dz;
  Real dist;
  Real ux, uy, uz;
  Real target_speed;
  Real desired_vx, desired_vy, desired_vz;

  Real dot;
  Real actual_target_qw, actual_target_qx, actual_target_qy, actual_target_qz;
  Real q_err_x, q_err_y, q_err_z;

equation
  // Propellant bookkeeping
  thrust = sqrt(force_x*force_x + force_y*force_y + force_z*force_z);
  der(m_prop) = -thrust / v_e;
  throttle = thrust / max_thrust;

  // 1. Spool-up/control lag filters (tau = 0.4s)
  der(filter_throttle) = (manual_throttle - filter_throttle) / 0.4;
  der(filter_pitch) = (manual_pitch - filter_pitch) / 0.4;
  der(filter_roll) = (manual_roll - filter_roll) / 0.4;
  der(filter_yaw) = (manual_yaw - filter_yaw) / 0.4;

  // 2. Position tracking equations (GNC)
  dx = target_x - pos_x;
  dy = (target_y + 5.0) - altitude; // 5.0m leg offset from ground-distance
  dz = target_z - pos_z;

  dist = sqrt(dx*dx + dy*dy + dz*dz);
  ux = if dist > 0.01 then dx / dist else 0.0;
  uy = if dist > 0.01 then dy / dist else 0.0;
  uz = if dist > 0.01 then dz / dist else 0.0;

  // Deceleration ramp within 5m
  target_speed = if dist >= 5.0 then speed
                 else speed * (dist / 5.0);

  desired_vx = ux * target_speed;
  desired_vy = uy * target_speed;
  desired_vz = uz * target_speed;

  // GNC (autonomous) world forces (including gravity feedforward on Y)
  gnc_force_x = vehicle_mass * kp_pos * (desired_vx - vel_x);
  gnc_force_y = vehicle_mass * kp_pos * (desired_vy - vel_y) + vehicle_mass * g;
  gnc_force_z = vehicle_mass * kp_pos * (desired_vz - vel_z);

  // 3. Rotation tracking equations (GNC)
  dot = target_qw*q_w + target_qx*q_x + target_qy*q_y + target_qz*q_z;
  actual_target_qw = if dot >= 0.0 then target_qw else -target_qw;
  actual_target_qx = if dot >= 0.0 then target_qx else -target_qx;
  actual_target_qy = if dot >= 0.0 then target_qy else -target_qy;
  actual_target_qz = if dot >= 0.0 then target_qz else -target_qz;

  // Vector part of relative rotation: q_err = q_target * q_current_inv
  q_err_x = actual_target_qx*q_w - actual_target_qw*q_x - actual_target_qy*q_z + actual_target_qz*q_y;
  q_err_y = actual_target_qy*q_w - actual_target_qw*q_y - actual_target_qz*q_x + actual_target_qx*q_z;
  q_err_z = actual_target_qz*q_w - actual_target_qw*q_z - actual_target_qx*q_y + actual_target_qy*q_x;

  // GNC (autonomous) world torques
  gnc_torque_x = 3000.0 * (kp_rot * q_err_x - kd_rot * w_x);
  gnc_torque_y = 3000.0 * (kp_rot * q_err_y - kd_rot * w_y);
  gnc_torque_z = 3000.0 * (kp_rot * q_err_z - kd_rot * w_z);

  // 4. Local manual force & torque generation
  f_loc_x = 0.0;
  f_loc_y = filter_throttle * max_thrust;
  f_loc_z = 0.0;

  // Torque strength (N*m) - 45000 N*m gives realistic control authority
  t_loc_x = filter_pitch * 45000.0;
  t_loc_y = filter_yaw * 45000.0;
  t_loc_z = filter_roll * 45000.0;

  // 5. Rotate local manual force to world frame
  f_world_x = 2.0 * (q_x*q_y + q_w*q_z) * f_loc_y;
  f_world_y = (1.0 - 2.0*(q_x*q_x + q_z*q_z)) * f_loc_y;
  f_world_z = 2.0 * (q_y*q_z - q_w*q_x) * f_loc_y;

  // 6. Rotate local manual torque to world frame
  t_world_x = t_loc_x + 2.0 * (q_y * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z) - q_z * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y));
  t_world_y = t_loc_y + 2.0 * (q_z * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x) - q_x * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z));
  t_world_z = t_loc_z + 2.0 * (q_x * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y) - q_y * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x));

  // 7. Select engine mode (GNC vs Manual override)
  force_x = engine_enable * (if manual > 0.5 then f_world_x else gnc_force_x);
  force_y = engine_enable * (if manual > 0.5 then f_world_y else gnc_force_y);
  force_z = engine_enable * (if manual > 0.5 then f_world_z else gnc_force_z);

  torque_x = engine_enable * (if manual > 0.5 then t_world_x else gnc_torque_x);
  torque_y = engine_enable * (if manual > 0.5 then t_world_y else gnc_torque_y);
  torque_z = engine_enable * (if manual > 0.5 then t_world_z else gnc_torque_z);
end Lander;
