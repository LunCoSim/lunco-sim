within LunCo.Propulsion;

// Reaction Control System (RCS / RKS) Attitude Control Thruster.
// Converts pulsed attitude control command u_rcs into thrust force F_rcs and propellant mass flow rate.
model RCSThruster
  parameter Real f_nom_n = 22.0 "Nominal RCS thruster output force, N";
  parameter Real isp_sec = 220.0 "Specific impulse, s";
  parameter Real g0 = 9.80665 "Standard gravity acceleration, m/s²";

  input Real u_rcs "RCS thruster pulse command, 0..1";
  output Real thrust_n "Output thrust force, N";
  output Real mass_flow_kgs "Propellant mass flow rate, kg/s";
equation
  thrust_n = f_nom_n * max(0.0, min(1.0, u_rcs));
  mass_flow_kgs = thrust_n / max(1.0, isp_sec * g0);
end RCSThruster;
