within LunCo.Electrical;
// The electrical side of a hub motor: a load on the bus. The MECHANICAL side is Avian's
// — the wheel's spin comes out of the physics step and the torque goes back in — so this
// model takes the commanded torque and the shaft speed as boundary inputs and reports
// what the motor must DRAW to hold them. `p.i > 0`: current enters from the node.
model DCMotor
  parameter Real efficiency = 0.85 "Electrical-to-mechanical efficiency, 0..1";
  parameter Real rated_power = 2000.0 "Continuous rated shaft power, W";

  input Real torque_cmd "Commanded shaft torque, N.m";
  input Real omega "Shaft speed, rad/s (from the physics step)";

  Pin p;
  Real mech_power "Mechanical power delivered, W";
equation
  mech_power = torque_cmd * omega;
  p.i = mech_power / (efficiency * p.v);
end DCMotor;
