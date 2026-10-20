within LunCo.Electrical;
// A battery on the bus: it sets the terminal voltage and integrates its own charge from
// whatever current flows through its pin. During discharge current leaves the pack, so
// `p.i < 0`; SoC falls when the loads outdraw the sources
// and rises when they do not — the balance is the circuit's, not a number anyone sums.
model Battery
  extends LunCo.Icons.Battery;
  parameter Real voltage_nom = 28.0 "Charge-averaged open-circuit voltage, V";
  parameter Real R_internal = 0.01 "Equivalent series resistance, Ohm";
  parameter Real capacity(unit="Ah") = 208.0 "Total capacity";
  parameter Real soc_init(unit="1") = 0.8 "State of charge at t=0, 0..1";
  parameter Real soc_empty_threshold(unit="1", min=1e-6) = 1e-3
    "Normalized charge reserve for the empty-voltage taper and boundary signal";

  Pin p annotation(Placement(transformation(extent={{90,-10},{110,10}})));
  Real soc(unit="1", start = soc_init, fixed = true, min = 0.0, max = 1.0) "State of charge, 0..1";
  output Real soc_out(unit="1") "State of charge, 0..1";
  output Real soc_percent(unit="%") "State of charge, percent";
  output Real capacity_ah(unit="Ah") "Authored total battery capacity";
  output Real charge_remaining_ah(unit="Ah") "Charge currently available";
  output Real terminal_voltage_v(unit="V") "Battery terminal voltage on the electrical bus";
  output Real terminal_current_a(unit="A") "Current into the battery; positive charges, negative discharges";
  output Real net_power_w(unit="W") "Electrical power into the battery; positive charges, negative discharges";
  output Real charge_power_w(unit="W") "Power absorbed from the bus, including heat; zero while supplying the bus";
  output Real discharge_power_w(unit="W") "Power currently supplied by the battery; zero while charging";
  output Real charge_current_a(unit="A") "Current entering the battery; positive while charging";
  output Real discharge_current_a(unit="A") "Current leaving the battery; positive while discharging";
  output Real open_circuit_voltage_v(unit="V") "Idealized storage voltage before series resistance";
  output Real stored_energy_j(unit="J") "Electrochemical energy remaining in the idealized storage law";
  output Real stored_power_w(unit="W") "Rate of change of stored energy; positive charges";
  output Real heat_w(unit="W") "Series loss and rejected full-charge power in the idealized shunt";
  output Real drained "1 in the authored empty reserve interval, otherwise 0";
  Real soc_rate(unit="1/s") "State-of-charge rate limited to the physical storage interval";
  Real bounded_soc(unit="1");
  Real plateau_voltage_v(unit="V");
  Real storage_current_a(unit="A");
equation
  assert(capacity > 0.0 and voltage_nom > 0.0 and R_internal >= 0.0
      and soc_init >= 0.0 and soc_init <= 1.0
      and soc_empty_threshold > 0.0 and soc_empty_threshold < 1.0,
    "Battery requires positive capacity and voltage, nonnegative resistance, initial SOC in [0,1] and reserve in (0,1)");
  // Mission-level idealization, not a measured cell OCV curve: a flat plateau
  // tapers to zero only across the explicit empty reserve. Normalize its
  // charge integral so full storage is exactly capacity * voltage_nom Wh.
  // A voltage proportional to SOC across the entire range would instead
  // halve that nameplate energy, even before circuit losses.
  bounded_soc = max(0.0, min(1.0, soc));
  // Keep invalid parameter expressions defined until the assertion above can
  // issue the owning diagnostic; invalid parameters never produce a valid run.
  plateau_voltage_v = if soc_empty_threshold > 0.0 and soc_empty_threshold < 1.0
    then voltage_nom / (1.0 - 0.5 * soc_empty_threshold) else 0.0;
  open_circuit_voltage_v = if soc_empty_threshold > 0.0 then plateau_voltage_v
    * min(1.0, bounded_soc / soc_empty_threshold) else 0.0;
  // Preserve the signed series-resistor law. An externally forced reverse
  // bus at empty absorbs power; clipping its voltage would hide that loss.
  p.v = open_circuit_voltage_v + p.i * R_internal;
  // Reject storage flux only at the actual bounds, rather than slowing the
  // charge state over an implicit one-second interval near either bound.
  storage_current_a = if soc <= 0.0 then max(0.0, p.i)
    else if soc >= 1.0 then min(0.0, p.i) else p.i;
  soc_rate = if capacity > 0.0 then storage_current_a / (capacity * 3600.0) else 0.0;
  der(soc) = soc_rate;
  stored_energy_j = capacity * 3600.0 * plateau_voltage_v
    * (if soc_empty_threshold <= 0.0 then 0.0 else if bounded_soc < soc_empty_threshold
      then bounded_soc^2 / (2.0 * soc_empty_threshold)
      else bounded_soc - 0.5 * soc_empty_threshold);
  stored_power_w = open_circuit_voltage_v * storage_current_a;
  // At full charge this simple model dissipates rejected charging power in
  // an ideal shunt. It does not represent a calibrated BMS or charge controller.
  heat_w = p.i^2 * R_internal + open_circuit_voltage_v * (p.i - storage_current_a);
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
  // fraction. Saturation bounds the continuous charge fraction while retaining
  // the exact boundary value: 1 at soc=0 and 0 at or above the authored 0.1%
  // empty reserve interval. The finite interval is intentional: it represents
  // a usable-storage reserve and keeps the event signal well-conditioned at
  // the constrained state boundary instead of relying on solver epsilon.
  drained = max(0.0, min(1.0, 1.0 - soc / soc_empty_threshold));
end Battery;
