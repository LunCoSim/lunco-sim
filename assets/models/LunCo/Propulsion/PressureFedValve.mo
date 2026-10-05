within LunCo.Propulsion;

model PressureFedValve
  "Opening-controlled liquid feed with passive valve/injector pressure loss"
  parameter Real maximum_flow_kgs = 4.0
    "Flow at full opening and nominal pressure drop (kg/s)";
  parameter Real commanded_flow_kgs = 4.0
    "Requested flow at command fraction one (kg/s)";
  parameter Real nominal_pressure_drop_pa = 1.0e6
    "Reference valve/injector differential pressure (Pa)";
  parameter Real opening_time_constant_s = 0.08
    "Lumped opening/closing response; not a resolved pulse model (s)";
  parameter Real availability_transition_mass_kg = 0.01;
  parameter Real minimum_pressure_drop_pa = 1.0;
  parameter Real minimum_time_constant_s = 1.0e-6;
  parameter Real minimum_available_mass_kg = 1.0e-6;

  input Real flow_fraction_command = 0.0 "Normalized requested liquid flow, 0..1";
  input Real available_mass_kg = 0.0 "Upstream available liquid (kg)";
  FluidPort inlet "Pressurized tank supply";
  FluidPort outlet "Chamber inlet after the valve and injector";
  output Real mass_flow_kgs "Delivered liquid flow (kg/s)";
  output Real outlet_pressure_pa "Downstream pressure (Pa)";
  output Real pressure_drop_pa "Supply minus downstream pressure (Pa)";
  output Real activity "Actual valve opening, 0..1";

  Real opening(start = 0.0, fixed = true);
  Real availability;
  Real flow_capacity_kgs;
  Real target_opening;

equation
  // Guidance requests thrust/flow, not valve travel. Compensate the passive
  // restriction for current pressure head before applying valve dynamics.
  // Saturation preserves the physical supply limit; depletion gates flow.
  target_opening = noEvent(max(0.0, min(1.0,
    max(0.0, min(1.0, flow_fraction_command)) * commanded_flow_kgs
      / max(1.0e-6, flow_capacity_kgs))));
  der(opening) = (target_opening - opening)
    / max(minimum_time_constant_s, opening_time_constant_s);
  activity = noEvent(max(0.0, min(1.0, opening)));
  availability = noEvent(max(0.0, min(1.0, available_mass_kg
    / max(minimum_available_mass_kg, availability_transition_mass_kg))));
  pressure_drop_pa = inlet.pressure_pa - outlet.pressure_pa;
  flow_capacity_kgs = maximum_flow_kgs
    * sqrt(noEvent(max(0.0, pressure_drop_pa))
      / max(minimum_pressure_drop_pa, nominal_pressure_drop_pa));
  mass_flow_kgs = flow_capacity_kgs * activity * availability;
  inlet.mass_flow_kgs = mass_flow_kgs;
  outlet.mass_flow_kgs = -mass_flow_kgs;
  // Passive restriction adds no shaft work or fictitious pressure rise.
  outlet.specific_enthalpy_j_kg = inStream(inlet.specific_enthalpy_j_kg);
  inlet.specific_enthalpy_j_kg = inStream(outlet.specific_enthalpy_j_kg);
  outlet_pressure_pa = outlet.pressure_pa;

  annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
    Polygon(points={{-70,45},{-70,-45},{0,0},{70,45},{70,-45},{0,0},{-70,45}},
      lineColor={65,85,105}, fillColor={175,190,205}, fillPattern=FillPattern.Solid),
    Line(points={{0,0},{0,70}}, color={65,85,105}, thickness=2),
    Rectangle(extent={{-20,70},{20,90}}, lineColor={65,85,105},
      fillColor={120,140,160}, fillPattern=FillPattern.Solid),
    Text(extent={{-90,-85},{90,-65}}, textString="%name", textColor={65,85,105})}));
end PressureFedValve;
