within LunCo.Propulsion;

connector FluidPort
  "Acausal single-phase propellant port"
  Real pressure_pa(unit = "Pa")
    "Shared line pressure at the connection node";
  flow Real mass_flow_kgs(unit = "kg/s")
    "Mass flow into the connected component";
  stream Real specific_enthalpy_j_kg(unit = "J/kg")
    "Specific enthalpy carried by fluid leaving the component";

  annotation(
    Icon(coordinateSystem(extent = {{-100, -100}, {100, 100}}), graphics = {
      Ellipse(
        extent = {{-72, -72}, {72, 72}},
        lineColor = {35, 120, 150},
        fillColor = {80, 185, 205},
        fillPattern = FillPattern.Solid),
      Line(points = {{-100, 0}, {-72, 0}}, color = {35, 120, 150}, thickness = 2),
      Line(points = {{72, 0}, {100, 0}}, color = {35, 120, 150}, thickness = 2),
      Text(extent = {{-55, -22}, {55, 22}}, textString = "P", textColor = {255, 255, 255}, fontSize = 26)
    }),
    Diagram(coordinateSystem(extent = {{-100, -100}, {100, 100}}), graphics = {
      Text(extent = {{-90, -90}, {90, -70}}, textString = "PROPELLANT LINE", textColor = {35, 120, 150}, fontSize = 9)
    }));
end FluidPort;
