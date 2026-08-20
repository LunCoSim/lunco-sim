within LunCo.Electrical;
// A battery on the bus: it sets the terminal voltage and integrates its own charge from
// whatever current flows through its pin. During discharge current leaves the pack, so
// `p.i < 0`; SoC falls when the loads outdraw the sources
// and rises when they do not — the balance is the circuit's, not a number anyone sums.
model Battery
  extends LunCo.Icons.Battery;
  parameter Real voltage_nom = 48.0 "Nominal terminal voltage, V";
  parameter Real R_internal = 0.01 "Equivalent series resistance, Ohm";
  parameter Real capacity(unit="Ah") = 208.0 "Total capacity";
  parameter Real soc_init(unit="1") = 0.8 "State of charge at t=0, 0..1";

  Pin p;
  Real soc(unit="1", start = soc_init) "State of charge, 0..1";
  output Real soc_out(unit="1") "State of charge, 0..1";
  output Real capacity_ah(unit="Ah") "Authored total battery capacity";
  output Real charge_remaining_ah(unit="Ah") "Charge currently available";
  output Real terminal_voltage_v(unit="V") "Battery terminal voltage on the electrical bus";
  output Real terminal_current_a(unit="A") "Current into the battery; positive charges, negative discharges";
  output Real terminal_power_w(unit="W") "Electrical power into the battery; positive charges, negative discharges";
equation
  // Terminal voltage droops with SoC and with the current drawn through the ESR.
  p.v = voltage_nom * (0.8 + 0.2 * soc) + p.i * R_internal;
  der(soc) = p.i / (capacity * 3600.0);
  soc_out = soc;
  capacity_ah = capacity;
  charge_remaining_ah = capacity * soc;
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
  terminal_power_w = p.v * p.i;
end Battery;
