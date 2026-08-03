within LunCo.Electrical;
// An ideal electrical rail for an explicitly unlimited-power configuration.
//
// This is a source, not a runtime special case: the enclosing USD network owns
// membership and connects this pin to its loads. The source fixes the bus
// voltage and lets Kirchhoff's current law determine the current it supplies.
// A battery-equipped vehicle uses Battery instead, while a laboratory rig or a
// video scene can deliberately select this ideal rail.
model IdealVoltageSource
  extends LunCo.Icons.ElectricalControl;
  parameter Real voltage = 48.0 "Fixed bus voltage, V";

  Pin p;
  output Real terminal_voltage_v(unit="V") "Voltage delivered to the bus";
  output Real terminal_current_a(unit="A") "Current supplied by the source";
  output Real terminal_power_w(unit="W") "Power delivered to the bus";
equation
  p.v = voltage;
  terminal_voltage_v = p.v;
  // Modelica's flow current is positive INTO the component. A load therefore
  // makes this source current negative; expose supplied current as positive.
  terminal_current_a = -p.i;
  terminal_power_w = voltage * terminal_current_a;
end IdealVoltageSource;
