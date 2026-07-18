within LunCo.Electrical;
// A battery on the bus: it sets the terminal voltage and integrates its own charge from
// whatever current flows through its pin. `p.i > 0` is discharge (current out of the pack
// into the bus is negative into the pin), so SoC falls when the loads outdraw the sources
// and rises when they do not — the balance is the circuit's, not a number anyone sums.
model Battery
  parameter Real voltage_nom = 48.0 "Nominal terminal voltage, V";
  parameter Real R_internal = 0.01 "Equivalent series resistance, Ohm";
  parameter Real capacity = 208.0 "Total capacity, Ah";
  parameter Real soc_init = 0.8 "State of charge at t=0, 0..1";

  Pin p;
  Real soc(start = soc_init) "State of charge, 0..1";
  output Real soc_out;
equation
  // Terminal voltage droops with SoC and with the current drawn through the ESR.
  p.v = voltage_nom * (0.8 + 0.2 * soc) - p.i * R_internal;
  der(soc) = -p.i / (capacity * 3600.0);
  soc_out = soc;
end Battery;
