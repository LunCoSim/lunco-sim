// tagline: Two-stage RC low-pass filter — 6 MSL blocks, renders as a schematic
model CascadedRCFilter
  "Two-stage RC low-pass filter — classic signal-conditioning example with six MSL components"

  // Tunable stage values. Each stage's cutoff is 1/(2·π·R·C).
  parameter Real V = 5.0 "Source voltage (V)";
  parameter Real R1_val = 1000.0 "Stage 1 resistance (Ω)";
  parameter Real C1_val = 1e-6 "Stage 1 capacitance (F)";
  parameter Real R2_val = 1000.0 "Stage 2 resistance (Ω)";
  parameter Real C2_val = 1e-6 "Stage 2 capacitance (F)";

  Modelica.Electrical.Analog.Sources.ConstantVoltage V_src(V=V) "Input voltage source";
  Modelica.Electrical.Analog.Basic.Resistor R1(R=R1_val) "Stage 1 series resistor";
  Modelica.Electrical.Analog.Basic.Capacitor C1(C=C1_val) "Stage 1 shunt capacitor";
  Modelica.Electrical.Analog.Basic.Resistor R2(R=R2_val) "Stage 2 series resistor";
  Modelica.Electrical.Analog.Basic.Capacitor C2(C=C2_val) "Stage 2 shunt capacitor (output)";
  Modelica.Electrical.Analog.Basic.Ground GND "Circuit ground";

equation
  connect(V_src.p, R1.p);
  connect(R1.n, C1.p);
  connect(C1.p, R2.p);
  connect(R2.n, C2.p);
  connect(C1.n, GND.p);
  connect(C2.n, GND.p);
  connect(V_src.n, GND.p);
end CascadedRCFilter;
