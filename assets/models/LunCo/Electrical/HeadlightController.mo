within LunCo.Electrical;

// Electrical and photometric facet of a reusable vehicle headlight. USD owns
// the lamp identity and mount; Rhai owns the enable policy; this model owns the
// continuous clamp, luminous output, and bus current demanded by the lamp.
model HeadlightController
  extends LunCo.Icons.ElectricalControl;
  parameter Real nominal_power_w = 24.0 "Full-on electrical draw, W";
  parameter Real luminous_intensity_lm = 120000.0 "Full-on luminous output, lm";

  input Real enable "Headlight command, 0 = off and 1 = on";
  output Real enabled "Clamped effective enable, 0..1";
  output Real light_intensity(unit="lm") "Luminous output for the USD light";
  output Real power_draw_w(unit="W") "Electrical power drawn from the bus";

  Pin p "Electrical bus pin";

equation
  enabled = max(0.0, min(1.0, enable));
  light_intensity = luminous_intensity_lm * enabled;
  power_draw_w = nominal_power_w * enabled;
  // Pin current is positive into a load. The bus voltage is supplied by the
  // enclosing source or battery network, so no voltage is duplicated here.
  p.i = power_draw_w / max(1.0, p.v);
end HeadlightController;
