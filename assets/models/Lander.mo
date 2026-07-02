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

  // ── External command inputs ──
  // Written by whoever holds control THIS tick — the player, or an autopilot
  // layer. Auto-vs-manual arbitration lives in that external layer, not here:
  // this model just actuates the commands it's given.
  input Real external_throttle = 0.0 "Vertical thrust command 0..1";
  input Real pitch = 0.0 "Pitch rate command -1..1";
  input Real roll = 0.0 "Roll rate command -1..1";
  input Real yaw = 0.0 "Yaw rate command -1..1";

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

  // NOTE: the autonomous GNC (position/attitude PD tracking) has moved OUT of
  // this model — auto guidance is now an external layer that writes the same
  // command inputs above. This model is a pure commands -> world force/torque
  // actuator. The GNC-only physics inputs (pos_*, vel_*, target_*, w_*, g, …)
  // and gains are retained but unused, so `simWires` need not change.

equation
  // Propellant bookkeeping
  thrust = sqrt(force_x*force_x + force_y*force_y + force_z*force_z);
  der(m_prop) = -thrust / v_e;
  throttle = thrust / max_thrust;

  // 1. Spool-up/control lag filters (tau = 0.4s)
  der(filter_throttle) = (external_throttle - filter_throttle) / 0.4;
  der(filter_pitch) = (pitch - filter_pitch) / 0.4;
  der(filter_roll) = (roll - filter_roll) / 0.4;
  der(filter_yaw) = (yaw - filter_yaw) / 0.4;

  // 2. Local command force & torque generation
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

  // 7. Actuate the commanded force/torque (gated only by engine_enable). Auto
  //    vs. manual is decided by whichever external layer writes the commands.
  force_x = engine_enable * f_world_x;
  force_y = engine_enable * f_world_y;
  force_z = engine_enable * f_world_z;

  torque_x = engine_enable * t_world_x;
  torque_y = engine_enable * t_world_y;
  torque_z = engine_enable * t_world_z;
end Lander;
