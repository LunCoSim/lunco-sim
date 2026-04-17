model RC_Circuit
  parameter Real V_source = 5 "Source voltage in Volts";
  parameter Real R = 100 "Series resistance in Ohms";
  parameter Real C = 0.001 "Capacitance in Farads";
  Modelica.Electrical.Analog.Sources.ConstantVoltage V1(V=V_source);
  Modelica.Electrical.Analog.Basic.Resistor R1(R=R);
  Modelica.Electrical.Analog.Basic.Capacitor C1(C=C);
  Modelica.Electrical.Analog.Basic.Ground GND;
equation
  connect(V1.p, R1.p);
  connect(R1.n, C1.p);
  connect(C1.n, GND.p);
  connect(V1.n, GND.p);
end RC_Circuit;
