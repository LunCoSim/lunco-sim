within LunCo.Electrical;
// The electrical side of a hub motor: a load on the bus. The MECHANICAL side is Avian's
// — the wheel's spin comes out of the physics step and the torque goes back in — so this
// coarse nameplate model takes normalized demand and reports the electrical draw.
// A higher-fidelity variant may replace it with a shaft-coupled machine without
// changing the public `p` connector or demand input.
model DCMotor
  parameter Real efficiency = 0.85 "Electrical-to-mechanical efficiency, 0..1";
  parameter Real rated_power = 2000.0 "Continuous rated shaft power, W";

  input Real demand "Normalized motor demand, -1..1";

  Pin p;
  Real electrical_power "Electrical power drawn, W";
equation
  electrical_power = rated_power * abs(demand) / max(0.01, efficiency);
  p.i = electrical_power / max(1.0, p.v);
end DCMotor;
