within;
// Rucheyok's electrical system: one circuit, solved as one DAE.
//
// This is the DOMAIN assembled — the panel, the pack and the four hub motors wired to a
// shared bus. It imports the reusable component models from `LunCo.Electrical` and does
// nothing but instantiate them, parameterize them, and connect them. Kirchhoff at the bus
// node balances source against load; nothing here counts the motors or sums a current.
//
// Its boundary is causal, and that is where cosim crosses: irradiance comes from the sun,
// each motor's shaft speed and torque from its wheel's physics step, and SoC goes back
// out. Inside the bus it is acausal; at the edge it is ports the rest of the twin wires.
//
// Parameter VALUES come from USD — the rover's `LunCoProgram "Electrical"` prim overrides
// these via `inputs:`. The topology (which components, connected how) is here because a
// circuit's shape is a Modelica model; USD authors the values and, in time, synthesizes
// the shape from the vehicle's electrical graph (doc 37).
model RucheyokElectrical
  import LunCo.Electrical.*;

  parameter Real panel_area = 72.0;
  parameter Real panel_efficiency = 0.32;
  parameter Real battery_capacity = 312.0;
  parameter Real battery_soc_init = 0.95;
  parameter Real motor_rated_power = 2000.0;

  Battery bat(capacity = battery_capacity, soc_init = battery_soc_init);
  SolarPanel panel(area = panel_area, efficiency = panel_efficiency);
  DCMotor m_fl(rated_power = motor_rated_power);
  DCMotor m_fr(rated_power = motor_rated_power);
  DCMotor m_rl(rated_power = motor_rated_power);
  DCMotor m_rr(rated_power = motor_rated_power);

  // Boundary — wired by cosim to the sun and the four wheels.
  input Real irradiance "W/m2, from the environment";
  input Real cos_incidence "sun angle onto the panel, 0..1";
  input Real torque_fl, torque_fr, torque_rl, torque_rr "N.m per wheel";
  input Real omega_fl, omega_fr, omega_rl, omega_rr "rad/s per wheel";
  output Real soc "battery state of charge, 0..1";
equation
  connect(panel.p, bat.p);
  connect(m_fl.p, bat.p);
  connect(m_fr.p, bat.p);
  connect(m_rl.p, bat.p);
  connect(m_rr.p, bat.p);

  panel.irradiance = irradiance;
  panel.cos_incidence = cos_incidence;
  m_fl.torque_cmd = torque_fl; m_fl.omega = omega_fl;
  m_fr.torque_cmd = torque_fr; m_fr.omega = omega_fr;
  m_rl.torque_cmd = torque_rl; m_rl.omega = omega_rl;
  m_rr.torque_cmd = torque_rr; m_rr.omega = omega_rr;

  soc = bat.soc_out;
end RucheyokElectrical;
