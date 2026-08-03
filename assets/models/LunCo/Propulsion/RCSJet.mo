within LunCo.Propulsion;

model RCSJet
  "One scalar RCS nozzle model; USD owns its physical mount"
  extends LunCo.Icons.RCSJet;

  parameter Real f_nom_n = 2500.0 "Nominal nozzle force (N)";
  parameter Real isp_sec = 220.0 "Specific impulse (s)";
  parameter Real g0 = 9.80665 "Standard gravity acceleration (m/s2)";
  parameter Real minimum_isp_g0 = 1.0e-6
    "Smallest specific-impulse/gravity product used for flow";

  input Real valve_opening "RCS valve opening, 0..1";
  output Real thrust_n "Nozzle thrust magnitude (N)";
  output Real mass_flow_kgs "Propellant flow (kg/s)";
  output Real activity "Normalized valve activity, 0..1";

  RCSThruster thruster(
    f_nom_n = f_nom_n,
    isp_sec = isp_sec,
    g0 = g0,
    minimum_isp_g0 = minimum_isp_g0);

equation
  thruster.valve_opening = valve_opening;
  thrust_n = thruster.thrust_n;
  mass_flow_kgs = thruster.mass_flow_kgs;
  activity = max(0.0, min(1.0, valve_opening));
end RCSJet;
