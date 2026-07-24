within LunCo.Electrical;
// A photovoltaic source on the bus. Its output is `area × efficiency × irradiance` at
// normal incidence, derated by the cosine of the sun angle and clamped to the lit
// hemisphere. It pushes that power onto the bus as current at the bus voltage — `p.i` is
// negative because current LEAVES the panel into the node.
model SolarPanel
  parameter Real area = 6.0 "Collecting area, m2";
  parameter Real efficiency = 0.30 "Irradiance-to-electrical conversion, 0..1";

  input Real irradiance "Incident irradiance, W/m2";
  input Real cos_incidence "Cosine of the sun incidence angle, 0..1";

  Pin p;
  output Real power_out "Electrical power generated, W";
equation
  power_out = area * efficiency * irradiance * max(0.0, cos_incidence);
  p.i = -power_out / p.v;
end SolarPanel;
