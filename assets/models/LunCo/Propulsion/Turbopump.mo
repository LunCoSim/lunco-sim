within LunCo.Propulsion;

model Turbopump
  "Valve-driven turbopump with spool dynamics and pressure rise"
  extends LunCo.Icons.Turbopump;

  parameter Real maximum_flow_kgs = 4.0;
  parameter Real nominal_tank_pressure_pa = 2.5e6;
  parameter Real discharge_pressure_pa = 8.0e6;
  parameter Real spool_tau = 0.08;
  parameter Real pump_efficiency = 0.72;
  parameter Real speed_rpm_max = 30000.0;
  parameter Real propellant_density_kg_m3 = 1000.0;
  parameter Real minimum_pressure_pa = 1.0;
  parameter Real minimum_efficiency = 1.0e-6;
  parameter Real minimum_time_constant_s = 1.0e-6;
  parameter Real minimum_available_mass_kg = 1.0e-6;
  parameter Real availability_transition_mass_kg = 0.01;

  input Real valve_opening = 0.0 "Commanded valve opening, 0..1";
  input Real available_mass_kg = 0.0 "Upstream propellant available to the pump, kg";
  FluidPort inlet "Propellant feed inlet";
  FluidPort outlet "Pressurised propellant outlet";
  output Real mass_flow_kgs "Delivered propellant flow, kg/s";
  output Real outlet_pressure_pa "Pump discharge pressure, Pa";
  output Real speed_fraction "Normalized pump speed";
  output Real speed_rpm "Normalized speed reported as rpm";
  output Real activity "Pump activity, 0..1";
  output Real shaft_power_w "Idealized pump shaft power, W";

  Real speed(start = 0.0);
  Real inlet_pressure_pa;
  Real availability;
  Real specific_pump_work_j_kg;

equation
  der(speed) = (max(0.0, min(1.0, valve_opening)) - speed)
    / max(minimum_time_constant_s, spool_tau);
  speed_fraction = max(0.0, min(1.0, speed));
  activity = speed_fraction;
  inlet_pressure_pa = inlet.pressure_pa;
  availability = max(0.0, min(1.0,
    available_mass_kg / max(minimum_available_mass_kg,
      availability_transition_mass_kg)));
  mass_flow_kgs = maximum_flow_kgs * activity
    * availability
    * sqrt(max(0.0, inlet_pressure_pa) / max(minimum_pressure_pa, nominal_tank_pressure_pa));
  inlet.mass_flow_kgs = mass_flow_kgs;
  outlet.mass_flow_kgs = -mass_flow_kgs;
  outlet.pressure_pa = max(inlet_pressure_pa,
    inlet_pressure_pa + (discharge_pressure_pa - nominal_tank_pressure_pa) * activity);
  specific_pump_work_j_kg = max(0.0, outlet.pressure_pa - inlet_pressure_pa)
    / max(minimum_efficiency, pump_efficiency * propellant_density_kg_m3);
  outlet.specific_enthalpy_j_kg = inStream(inlet.specific_enthalpy_j_kg)
    + specific_pump_work_j_kg;
  inlet.specific_enthalpy_j_kg = inStream(outlet.specific_enthalpy_j_kg);
  outlet_pressure_pa = outlet.pressure_pa;
  speed_rpm = speed_rpm_max * speed_fraction;
  shaft_power_w = max(0.0, mass_flow_kgs * specific_pump_work_j_kg);
end Turbopump;
