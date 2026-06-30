// tagline: Lander GNC — 3D speed-limited position + quaternion attitude PD control
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
  parameter Real kp_rot = 2.0 "Rotation proportional gain";
  parameter Real kd_rot = 1.5 "Rotation derivative gain";

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

  // 1. Position tracking equations
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

  // Apply forces (including gravity feedforward on Y)
  force_x = engine_enable * vehicle_mass * kp_pos * (desired_vx - vel_x);
  force_y = engine_enable * (vehicle_mass * kp_pos * (desired_vy - vel_y) + vehicle_mass * g);
  force_z = engine_enable * vehicle_mass * kp_pos * (desired_vz - vel_z);

  // 2. Rotation tracking equations
  dot = target_qw*q_w + target_qx*q_x + target_qy*q_y + target_qz*q_z;
  actual_target_qw = if dot >= 0.0 then target_qw else -target_qw;
  actual_target_qx = if dot >= 0.0 then target_qx else -target_qx;
  actual_target_qy = if dot >= 0.0 then target_qy else -target_qy;
  actual_target_qz = if dot >= 0.0 then target_qz else -target_qz;

  // Vector part of relative rotation: q_err = q_target * q_current_inv
  q_err_x = actual_target_qx*q_w - actual_target_qw*q_x - actual_target_qz*q_y + actual_target_qy*q_z;
  q_err_y = actual_target_qy*q_w + actual_target_qz*q_x - actual_target_qw*q_y - actual_target_qx*q_z;
  q_err_z = actual_target_qz*q_w - actual_target_qy*q_x + actual_target_qx*q_y - actual_target_qw*q_z;

  // Command world torques (principal inertia ≈ 3000)
  torque_x = engine_enable * 3000.0 * (kp_rot * q_err_x - kd_rot * w_x);
  torque_y = engine_enable * 3000.0 * (kp_rot * q_err_y - kd_rot * w_y);
  torque_z = engine_enable * 3000.0 * (kp_rot * q_err_z - kd_rot * w_z);
end Lander;
