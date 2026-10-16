within LunCo.Mobility;
block RoverCommandAuthority
  "Select session, direct program, or waypoint authority on one rover command surface"
  input Real piloted "External session authority";
  input Real program_active "Authored program authority";
  input Real autopilot_enable "Select waypoint guidance within the program";
  output Real direct_gate "Weight of the throttle/steer setpoint";
  output Real guidance_gate "Weight of the waypoint controller";
protected
  Real session_weight;
  Real program_weight;
  Real waypoint_weight;
equation
  session_weight = max(0.0, min(1.0, piloted));
  program_weight = max(0.0, min(1.0, program_active));
  waypoint_weight = max(0.0, min(1.0, autopilot_enable));
  guidance_gate = program_weight * waypoint_weight;
  direct_gate = session_weight * (1.0 - program_weight)
    + program_weight * (1.0 - waypoint_weight);
end RoverCommandAuthority;
