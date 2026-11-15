within LunCo.Electrical;
partial model RegulatedLoad
  "Ideal regulated load with undervoltage lockout and restart hysteresis"
  parameter Real voltage_off_v(unit="V") = 18.0
    "UVLO turn-off threshold; illustrative 28 V equipment default, author for the device";
  parameter Real voltage_on_v(unit="V") = 20.0
    "Restart threshold above turn-off; illustrative 28 V equipment default";
  output Real powered "1 while the input converter is operating, otherwise 0";
  output Real power_draw_w(unit="W") "Actual electrical input power at the pin";
  Pin p "Positive bus terminal relative to the network's electrical reference" annotation(Placement(transformation(origin={-100,0}, extent={{-10,-10},{10,10}})));
protected
  Boolean operating(start=false, fixed=true);
  Real requested_power_w(unit="W") "On-state regulated input demand";
equation
  assert(voltage_off_v > 0.0 and voltage_on_v > voltage_off_v,
    "RegulatedLoad requires 0 < voltage_off_v < voltage_on_v");
  assert(requested_power_w >= 0.0, "RegulatedLoad requires nonnegative input demand");
  // Latch at voltage crossings: disconnecting a load can rebound the bus
  // above the off threshold without reaching the distinct restart threshold.
  when {initial(), p.v <= voltage_off_v, p.v >= voltage_on_v} then
    operating = p.v >= voltage_on_v;
  end when;
  powered = if operating then 1.0 else 0.0;
  // The hysteresis equation owns switching. The denominator regularization
  // stays finite during inactive algebraic evaluations and creates no second
  // event surface at the same UVLO boundary. An open converter draws zero.
  p.i = powered * requested_power_w / noEvent(max(voltage_off_v, p.v));
  power_draw_w = p.v * p.i;
end RegulatedLoad;
