within LunCo.Propulsion;

// Reaction Control System (RCS / RKS) Attitude Control Thruster.
// Converts a valve opening into thrust force and propellant mass flow rate.
model RCSThruster
  extends LunCo.Icons.RCSThruster;
  parameter Real f_nom_n = 22.0 "Nominal RCS thruster output force, N";
  parameter Real isp_sec = 220.0 "Specific impulse, s";
  parameter Real g0 = 9.80665 "Standard gravity acceleration, m/s²";
  parameter Real minimum_isp_g0 = 1.0e-6
    "Smallest specific-impulse/gravity product used for flow";

  input Real valve_opening "RCS valve opening, 0..1";
  output Real thrust_n "Output thrust force, N";
  output Real mass_flow_kgs "Propellant mass flow rate, kg/s";
equation
  thrust_n = f_nom_n * max(0.0, min(1.0, valve_opening));
  mass_flow_kgs = thrust_n / max(minimum_isp_g0, isp_sec * g0);
end RCSThruster;
