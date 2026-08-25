within LunCo.Electrical;
// A two-domain DC motor at the Modelica/engine mechanical boundary. The electrical
// Pin and measured shaft-speed input determine winding current, back-EMF, torque,
// heat, and power. The generic Torque member places that solved torque on the
// authored rotational network; the engine-side wheel owns the rotational state and
// its authored total inertia, so no state is integrated twice across the boundary.
model DCMotor
  extends LunCo.Icons.Motor;
  parameter Real stall_torque = 1.5 "Motor-shaft stall torque, N.m";
  parameter Real no_load_speed = 4800.0 "Motor-shaft no-load speed, rad/s";
  parameter Real rated_power = 500.0 "Continuous electrical nameplate power, W";
  parameter Real v_rated = 28.0 "Bus voltage the drive is rated at, V";

  input Real demand "Normalized motor demand, -1..1";
  input Real speed(unit="rad/s") "Measured shaft speed from the mechanical engine";

  Pin p;

  output Real electrical_power(unit="W") "Electrical power drawn by the motor drive";
  output Real heat(unit="W") "Electrical loss delivered to the thermal network";
  output Real terminal_voltage_v(unit="V") "Voltage supplied to the motor drive";
  output Real terminal_current_a(unit="A") "Current drawn by the motor drive";
  output Real mechanical_power_w(unit="W") "Solved shaft mechanical power";
  output Real shaft_torque(unit="N.m") "Solved torque delivered to the shaft";
  output Real shaft_speed(unit="rad/s") "Solved motor-shaft speed";

  Real winding_voltage(unit="V");
  Real winding_current(unit="A");
  Real back_emf(unit="V");
  Real resistance(unit="Ohm");
  Real torque_constant(unit="N.m/A");
  Real back_emf_constant(unit="V.s/rad");
equation
  // The nameplate derives the winding constants once. No battery or source is
  // named here: any authored electrical network determines p.v and p.i.
  resistance = v_rated * v_rated / rated_power;
  torque_constant = stall_torque / (rated_power / v_rated);
  back_emf_constant = v_rated / no_load_speed;

  shaft_speed = speed;
  winding_voltage = demand * p.v;
  back_emf = back_emf_constant * shaft_speed;
  winding_current = (winding_voltage - back_emf) / resistance;

  // An ideal duty-controlled drive transforms bus current into winding
  // current. The product p.v*p.i therefore equals winding_voltage*winding_current.
  p.i = demand * winding_current;
  shaft_torque = torque_constant * winding_current;

  electrical_power = p.i * p.v;
  heat = resistance * winding_current * winding_current;
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
  mechanical_power_w = shaft_torque * shaft_speed;
end DCMotor;
