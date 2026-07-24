within LunCo.GNC;

// E-Guidance / Optimal Powered Descent Guidance (PDG) for precision lander touchdown.
// Calculates required thrust acceleration vector to guide a fast-moving lander to a target landing point.
// Acceleration law: a_cmd = (6/t_go²) * (p_target - p - v*t_go) - (2/t_go) * (v_target - v) + g_lunar
model PoweredDescentGuidance
  parameter Real g_lunar = 1.62 "Lunar gravity acceleration, m/s²";
  parameter Real a_max = 12.0 "Maximum engine thrust acceleration capacity, m/s²";

  // Lander state inputs from sensors / navigation
  input Real pos_x "Lander position X, m";
  input Real pos_y "Lander position Y, m";
  input Real pos_z "Lander altitude Z, m";

  input Real vel_x "Lander velocity X, m/s";
  input Real vel_y "Lander velocity Y, m/s";
  input Real vel_z "Lander vertical velocity Z, m/s";

  // Target landing site coordinates
  input Real target_x = 0.0 "Target landing position X, m";
  input Real target_y = 0.0 "Target landing position Y, m";
  input Real target_z = 0.0 "Target landing altitude Z, m";

  // Guidance outputs to main engine & attitude thrusters
  output Real t_go_sec "Estimated time-to-go until touchdown, s";
  output Real a_req_z "Required vertical acceleration, m/s²";
  output Real throttle_cmd "Main engine throttle command, 0..1";
  output Real pitch_cmd_deg "Commanded pitch attitude angle, deg";

  Real dist_to_target "Distance to target site, m";
  Real speed "Current lander speed, m/s";
equation
  dist_to_target = sqrt(max(0.01, (target_x - pos_x)^2 + (target_y - pos_y)^2 + (target_z - pos_z)^2));
  speed = sqrt(max(0.01, vel_x^2 + vel_y^2 + vel_z^2));

  // Time-to-go estimation: t_go = 2 * dist / max(1.0, speed)
  t_go_sec = max(2.0, 2.0 * dist_to_target / max(1.0, speed));

  // Required vertical acceleration for soft touchdown
  a_req_z = (6.0 / (t_go_sec^2)) * (target_z - pos_z - vel_z * t_go_sec) + (2.0 / t_go_sec) * vel_z + g_lunar;

  // Throttle command normalized to engine maximum acceleration
  throttle_cmd = max(0.1, min(1.0, a_req_z / max(1.0, a_max)));

  // Pitch angle for lateral velocity vector cancellation
  pitch_cmd_deg = max(-45.0, min(45.0, (vel_x / max(1.0, abs(vel_z))) * 57.2958));
end PoweredDescentGuidance;
