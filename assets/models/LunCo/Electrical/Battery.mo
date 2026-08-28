within LunCo.Electrical;
// A battery on the bus: it sets the terminal voltage and integrates its own charge from
// whatever current flows through its pin. During discharge current leaves the pack, so
// `p.i < 0`; SoC falls when the loads outdraw the sources
// and rises when they do not — the balance is the circuit's, not a number anyone sums.
model Battery
  extends LunCo.Icons.Battery;
  parameter Real voltage_nom = 28.0 "Nominal terminal voltage, V";
  parameter Real R_internal = 0.01 "Equivalent series resistance, Ohm";
  parameter Real capacity(unit="Ah") = 208.0 "Total capacity";
  parameter Real soc_init(unit="1") = 0.8 "State of charge at t=0, 0..1";
  parameter Real soc_empty_threshold(unit="1", min=1e-6) = 1e-3
    "Normalized SoC reserve interval used to expose the empty-boundary signal";

  Pin p annotation(Placement(transformation(extent={{90,-10},{110,10}})));
  Real soc(unit="1", start = soc_init, fixed = true, min = 0.0, max = 1.0) "State of charge, 0..1";
  output Real soc_out(unit="1") "State of charge, 0..1";
  output Real soc_percent(unit="%") "State of charge, percent";
  output Real capacity_ah(unit="Ah") "Authored total battery capacity";
  output Real charge_remaining_ah(unit="Ah") "Charge currently available";
  output Real terminal_voltage_v(unit="V") "Battery terminal voltage on the electrical bus";
  output Real terminal_current_a(unit="A") "Current into the battery; positive charges, negative discharges";
  output Real net_power_w(unit="W") "Electrical power into the battery; positive charges, negative discharges";
  output Real charge_power_w(unit="W") "Power currently stored in the battery; zero while discharging";
  output Real discharge_power_w(unit="W") "Power currently supplied by the battery; zero while charging";
  output Real charge_current_a(unit="A") "Current entering the battery; positive while charging";
  output Real discharge_current_a(unit="A") "Current leaving the battery; positive while discharging";
  output Real drained "1 in the authored empty reserve interval, otherwise 0";
  Real soc_rate(unit="1/s") "State-of-charge rate limited to the physical storage interval";
equation
  // Terminal voltage droops with SoC and with the current drawn through the ESR.
  // The usable bus potential follows the stored fraction. At empty the
  // storage element contributes no voltage; any remaining bus potential must
  // come from another authored source on the electrical node.
  p.v = max(0.0, voltage_nom * max(0.0, min(1.0, soc)) + p.i * R_internal);
  soc_rate = p.i / (capacity * 3600.0);
  // A finite battery cannot store less than empty or more than full. The
  // current remains the solved terminal current, while only the storage state
  // is prevented from leaving its physical interval at either boundary.
  der(soc) = max(-soc, min(1.0 - soc, soc_rate));
  soc_out = soc;
  soc_percent = 100.0 * soc;
  capacity_ah = capacity;
  charge_remaining_ah = capacity * soc;
  terminal_voltage_v = p.v;
  terminal_current_a = p.i;
  net_power_w = p.v * p.i;
  charge_power_w = max(0.0, net_power_w);
  discharge_power_w = max(0.0, -net_power_w);
  charge_current_a = max(0.0, p.i);
  discharge_current_a = max(0.0, -p.i);
  // Event bindings consume a normalized empty-boundary signal, not a depletion
  // fraction. Saturation keeps it branch-free for the solver while retaining
  // the exact boundary value: 1 at soc=0 and 0 at or above the authored 0.1%
  // empty reserve interval. The finite interval is intentional: it represents
  // a usable-storage reserve and keeps the event signal well-conditioned at
  // the constrained state boundary instead of relying on solver epsilon.
  drained = max(0.0, min(1.0, 1.0 - soc / soc_empty_threshold));
end Battery;
