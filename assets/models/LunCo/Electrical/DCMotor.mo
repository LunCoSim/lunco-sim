within LunCo.Electrical;
// The electrical side of a hub motor: a load on the bus. The MECHANICAL side is Avian's
// — the wheel's spin comes out of the physics step and the torque goes back in — so this
// coarse nameplate model takes normalized demand and reports the electrical draw.
// A higher-fidelity variant may replace it with a shaft-coupled machine without
// changing the public `p` connector or demand input.
model DCMotor
  extends LunCo.Icons.Motor;
  parameter Real efficiency = 0.85 "Electrical-to-mechanical efficiency, 0..1";
  parameter Real rated_power = 500.0 "Continuous electrical nameplate power, W";
  // Bus voltage the drive is rated at. Used to turn the nameplate power rating
  // into the rated CURRENT the controller commands — a property of the machine's
  // rating, not a reading of the node.
  parameter Real v_rated = 28.0 "Bus voltage the drive is rated at, V";

  input Real demand "Normalized motor demand, -1..1";

  Pin p;
  // OUTPUT, not a plain `Real`, and that is what makes it observable: the domain
  // projection publishes a member's `output` variables as ports on the island and
  // nothing else, so a bare `Real` is computed every step and readable by no one.
  // A reported quantity is not a causal claim — `p.i` and `p.v` are still solved
  // acausally by the connection set; this only says the number leaves the model.
  output Real electrical_power(unit="W") "Electrical power drawn by the motor drive";
  output Real heat(unit="W") "Electrical loss delivered to the thermal network";
  output Real terminal_voltage_v(unit="V") "Voltage supplied to the motor drive";
  output Real terminal_current_a(unit="A") "Current drawn by the motor drive";
  output Real mechanical_power_w(unit="W") "Estimated mechanical power available after electrical losses";
  output Real available_demand "Demand after the bus voltage limits motor authority";
equation
  // A motor cannot produce its commanded torque without terminal voltage. The
  // normalized command is therefore limited by the solved bus potential. This
  // is a motor equation, not a battery case: any source, storage device, or
  // domain that changes `p.v` changes the available mechanical authority.
  available_demand = demand * max(0.0, min(1.0, p.v / v_rated));
  p.i = (rated_power / v_rated) * abs(available_demand) / max(0.01, efficiency);

  // Power drawn FOLLOWS from the current and the voltage actually present.
  electrical_power = p.i * p.v;
  // The electrical loss is a physical observable, not a second command path.
  // A generated thermal assembly feeds this into `LunCo.Thermal.HeatLoad`,
  // which injects it into an acausal heat-port network of masses and radiators.
  heat = max(0.0, electrical_power) * (1.0 - efficiency);
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
  mechanical_power_w = max(0.0, electrical_power) * efficiency;
end DCMotor;
