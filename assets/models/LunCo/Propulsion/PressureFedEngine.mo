within LunCo.Propulsion;

model PressureFedEngine
  "One pressure-fed bipropellant engine: fuel and oxidizer valves feeding a choked chamber"
  extends LunCo.Icons.CombustionChamber;

  parameter Real fuel_maximum_flow_kgs = 0.3
    "Fuel valve flow at full opening and nominal pressure drop (kg/s)";
  parameter Real fuel_commanded_flow_kgs = 0.3
    "Fuel flow requested at command fraction one (kg/s)";
  parameter Real oxidizer_maximum_flow_kgs = 0.75
    "Oxidizer valve flow at full opening and nominal pressure drop (kg/s)";
  parameter Real oxidizer_commanded_flow_kgs = 0.75
    "Oxidizer flow requested at command fraction one (kg/s)";
  parameter Real nominal_pressure_drop_pa = 1.0e6
    "Reference valve/injector differential pressure (Pa)";
  parameter Real opening_time_constant_s = 0.08
    "Lumped valve opening/closing response (s)";
  parameter Real availability_transition_mass_kg = 0.01
    "Upstream mass over which a draining tank stops feeding (kg)";
  parameter Real oxidizer_to_fuel_ratio = 2.6;
  parameter Real characteristic_velocity_mps = 1550.0;
  parameter Real throat_area_m2 = 6.4e-4;
  parameter Real combustion_efficiency = 0.96;
  parameter Real nominal_flow_kgs = 1.05
    "Propellant flow of this engine at full command (kg/s)";
  parameter Real chamber_temperature_full_k = 3300.0;
  parameter Real fuel_energy_j_kg = 4.0e7;

  input Real flow_fraction_command = 0.0 "Normalized requested propellant flow, 0..1";
  input Real enabled = 1.0
    "Engine health, 0..1: 0 is a failed engine whose valves cannot open";
  input Real available_fuel_mass_kg = 0.0 "Upstream fuel available to this engine (kg)";
  input Real available_oxidizer_mass_kg = 0.0
    "Upstream oxidizer available to this engine (kg)";
  input Real effective_exhaust_velocity_mps = 2940.0
    "Nominal nozzle effective velocity, supplied by a connected design model";

  FluidPort fuel_in "Pressurized fuel supply";
  FluidPort oxidizer_in "Pressurized oxidizer supply";

  output Real thrust_n "Delivered thrust (N)";
  output Real maximum_thrust_n "Thrust capability at nominal flow (N)";
  output Real propellant_flow "Propellant flow through the chamber (kg/s)";
  output Real fuel_flow_kgs "Fuel flow through this engine (kg/s)";
  output Real oxidizer_flow_kgs "Oxidizer flow through this engine (kg/s)";
  output Real activity "Combustion activity, 0..1";
  output Real chamber_pressure_pa "Chamber pressure (Pa)";
  output Real exhaust_velocity_mps "Current effective exhaust velocity (m/s)";
  output Real design_exhaust_velocity_mps "Nominal effective exhaust velocity (m/s)";
  output Real mixture_ratio "Oxidizer/fuel mixture ratio";
  output Real mixture_efficiency "Efficiency factor from mixture-ratio error";

  PressureFedValve fuel_valve(
    maximum_flow_kgs = fuel_maximum_flow_kgs,
    commanded_flow_kgs = fuel_commanded_flow_kgs,
    nominal_pressure_drop_pa = nominal_pressure_drop_pa,
    opening_time_constant_s = opening_time_constant_s,
    availability_transition_mass_kg = availability_transition_mass_kg);
  PressureFedValve oxidizer_valve(
    maximum_flow_kgs = oxidizer_maximum_flow_kgs,
    commanded_flow_kgs = oxidizer_commanded_flow_kgs,
    nominal_pressure_drop_pa = nominal_pressure_drop_pa,
    opening_time_constant_s = opening_time_constant_s,
    availability_transition_mass_kg = availability_transition_mass_kg);
  PressureFedCombustionChamber chamber(
    oxidizer_to_fuel_ratio = oxidizer_to_fuel_ratio,
    characteristic_velocity_mps = characteristic_velocity_mps,
    throat_area_m2 = throat_area_m2,
    combustion_efficiency = combustion_efficiency,
    nominal_flow_kgs = nominal_flow_kgs,
    chamber_temperature_full_k = chamber_temperature_full_k,
    fuel_energy_j_kg = fuel_energy_j_kg);

equation
  // A failed engine is a health input on the command path, so its valves
  // close through their own opening dynamics and the shared tanks see the
  // reduced demand through the acausal feed.
  fuel_valve.flow_fraction_command = flow_fraction_command
    * noEvent(max(0.0, min(1.0, enabled)));
  oxidizer_valve.flow_fraction_command = flow_fraction_command
    * noEvent(max(0.0, min(1.0, enabled)));
  fuel_valve.available_mass_kg = available_fuel_mass_kg;
  oxidizer_valve.available_mass_kg = available_oxidizer_mass_kg;
  chamber.effective_exhaust_velocity_mps = effective_exhaust_velocity_mps;
  connect(fuel_in, fuel_valve.inlet);
  connect(oxidizer_in, oxidizer_valve.inlet);
  connect(fuel_valve.outlet, chamber.fuel_in);
  connect(oxidizer_valve.outlet, chamber.oxidizer_in);
  thrust_n = chamber.thrust_n;
  maximum_thrust_n = chamber.maximum_thrust_n;
  propellant_flow = chamber.propellant_flow;
  fuel_flow_kgs = fuel_valve.mass_flow_kgs;
  oxidizer_flow_kgs = oxidizer_valve.mass_flow_kgs;
  activity = chamber.activity;
  chamber_pressure_pa = chamber.chamber_pressure_pa;
  exhaust_velocity_mps = chamber.exhaust_velocity_mps;
  design_exhaust_velocity_mps = chamber.design_exhaust_velocity_mps;
  mixture_ratio = chamber.mixture_ratio;
  mixture_efficiency = chamber.mixture_efficiency;
end PressureFedEngine;
