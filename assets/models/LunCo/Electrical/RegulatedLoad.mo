within LunCo.Electrical;
partial model RegulatedLoad
  "Ideal regulated load with undervoltage lockout and restart hysteresis"
  parameter Real voltage_off_v(unit="V") = 18.0
    "UVLO turn-off threshold; illustrative 28 V equipment default, author for the device";
  parameter Real voltage_on_v(unit="V") = 20.0
    "Restart threshold above turn-off; illustrative 28 V equipment default";
  output Real powered "1 while the input converter is operating, otherwise 0";
  output Real power_draw_w(unit="W") "Actual electrical input power at the pin";
  Pin p "Positive bus terminal relative to the network's electrical reference";
protected
  Boolean operating(start=false, fixed=true);
  Real requested_power_w(unit="W") "On-state regulated input demand";
equation
  assert(voltage_off_v > 0.0 and voltage_on_v > voltage_off_v,
    "RegulatedLoad requires 0 < voltage_off_v < voltage_on_v");
  assert(requested_power_w >= 0.0, "RegulatedLoad requires nonnegative input demand");
  operating = p.v >= voltage_on_v or (pre(operating) and p.v > voltage_off_v);
  powered = if operating and p.v > voltage_off_v then 1.0 else 0.0;
  // The conducting branch is valid only above UVLO. Its inactive algebraic
  // evaluation remains finite during voltage events; no fictitious pin power
  // is introduced because the open converter draws exactly zero current.
  p.i = powered * requested_power_w / max(voltage_off_v, p.v);
  power_draw_w = p.v * p.i;
end RegulatedLoad;
