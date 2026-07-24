within LunCo.GNC;

// RCS Thruster Command Allocation & Boresight Mapping Matrix Model.
// Translates 3D force (F_x, F_y, F_z) and torque (tau_x, tau_y, tau_z) control demands
// into individual RCS thruster nozzle pulse commands (u_1..u_4) based on 3D mount vectors.
model ThrusterMapper
  parameter Real f_nom_n = 22.0 "Nominal RCS thruster force, N";
  parameter Real arm_r_m = 0.8 "Thruster mount moment arm distance from CoM, m";

  // Control force and torque inputs from GNC Guidance
  input Real f_cmd_z "Commanded vertical deceleration force, N";
  input Real tau_cmd_x "Commanded pitch control torque, N.m";
  input Real tau_cmd_y "Commanded yaw control torque, N.m";
  input Real tau_cmd_z "Commanded roll control torque, N.m";

  // Pulse width duty cycle output commands to 4 RCS thruster nozzles
  output Real u_thruster_1 "RCS Nozzle +X pulse command, 0..1";
  output Real u_thruster_2 "RCS Nozzle -X pulse command, 0..1";
  output Real u_thruster_3 "RCS Nozzle +Y pulse command, 0..1";
  output Real u_thruster_4 "RCS Nozzle -Y pulse command, 0..1";
equation
  u_thruster_1 = max(0.0, min(1.0, (f_cmd_z * 0.25 + tau_cmd_x / (2.0 * arm_r_m)) / f_nom_n));
  u_thruster_2 = max(0.0, min(1.0, (f_cmd_z * 0.25 - tau_cmd_x / (2.0 * arm_r_m)) / f_nom_n));
  u_thruster_3 = max(0.0, min(1.0, (f_cmd_z * 0.25 + tau_cmd_y / (2.0 * arm_r_m)) / f_nom_n));
  u_thruster_4 = max(0.0, min(1.0, (f_cmd_z * 0.25 - tau_cmd_y / (2.0 * arm_r_m)) / f_nom_n));
end ThrusterMapper;
