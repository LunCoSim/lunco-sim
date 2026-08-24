within LunCo.Electrical;
// A two-domain DC motor. The electrical Pin and mechanical Flange are one solved
// component: bus voltage drives the winding, shaft speed produces back-EMF, and
// the resulting torque is applied to the same flange that carries the load back
// into the electrical equations.
model DCMotor
  extends LunCo.Icons.Motor;
  parameter Real stall_torque = 1.5 "Motor-shaft stall torque, N.m";
  parameter Real no_load_speed = 4800.0 "Motor-shaft no-load speed, rad/s";
  parameter Real rotor_inertia = 0.00012 "Motor rotor inertia, kg.m2";
  parameter Real rated_power = 500.0 "Continuous electrical nameplate power, W";
  parameter Real v_rated = 28.0 "Bus voltage the drive is rated at, V";

  input Real demand "Normalized motor demand, -1..1";

  Pin p;
  LunCo.Mechanics.Flange shaft;

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

  shaft_speed = der(shaft.phi);
  winding_voltage = demand * p.v;
  back_emf = back_emf_constant * shaft_speed;
  winding_current = (winding_voltage - back_emf) / resistance;

  // An ideal duty-controlled drive transforms bus current into winding
  // current. The product p.v*p.i therefore equals winding_voltage*winding_current.
  p.i = demand * winding_current;
  shaft_torque = torque_constant * winding_current;
  // Flange.tau is torque INTO the flange. The motor applies the opposite sign
  // to the connected load, while the flange flow carries the reaction back.
  shaft.tau = -shaft_torque;

  electrical_power = p.i * p.v;
  heat = resistance * winding_current * winding_current;
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
  mechanical_power_w = shaft_torque * shaft_speed;
end DCMotor;
