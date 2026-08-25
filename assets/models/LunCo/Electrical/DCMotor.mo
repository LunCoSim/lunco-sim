within LunCo.Electrical;
// The electrical facet of a DC motor drive.
//
// The motor's shaft, reduction and wheel contact are owned by the USD/Avian
// drivetrain. This Modelica class owns only the electrical load presented to
// the bus for the authored demand. A rotational Flange here would create a
// second mechanical owner: generated electrical islands do not contain the
// wheel or an Avian boundary, so that free connector would make the island
// structurally singular.
model DCMotor
  extends LunCo.Icons.Motor;
  parameter Real rated_power = 500.0 "Continuous electrical nameplate power, W";
  parameter Real v_rated = 28.0 "Bus voltage the drive is rated at, V";

  input Real demand "Normalized motor demand, -1..1";

  Pin p;

  output Real electrical_power(unit="W") "Electrical power drawn by the motor drive";
  output Real heat(unit="W") "Electrical loss delivered to the thermal network";
  output Real terminal_voltage_v(unit="V") "Voltage supplied to the motor drive";
  output Real terminal_current_a(unit="A") "Current drawn by the motor drive";
  output Real available_demand "Demand admitted by the solved electrical bus";

  Real winding_voltage(unit="V");
  Real winding_current(unit="A");
  Real resistance(unit="Ohm");
  Real requested_demand;
equation
  // The nameplate derives the winding resistance once. No battery or source is
  // named here: any authored electrical network determines p.v and p.i.
  resistance = v_rated * v_rated / rated_power;
  requested_demand = max(-1.0, min(1.0, demand));
  // The solved bus voltage is the admission boundary. A brownout cannot leave
  // a commanded current source drawing power or producing winding heat against
  // a dead bus; demand is reduced continuously from zero to nameplate voltage.
  available_demand = requested_demand * max(0.0, min(1.0, p.v / v_rated));
  winding_voltage = available_demand * v_rated;
  winding_current = winding_voltage / resistance;

  // Current is the magnitude of the admitted winding current because the
  // drive consumes bus power for either signed direction of shaft demand.
  // This remains an electrical facet: torque, speed and contact mechanics are
  // still owned by USD/Avian.
  p.i = abs(winding_current);

  electrical_power = p.i * p.v;
  heat = resistance * winding_current * winding_current;
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
end DCMotor;
