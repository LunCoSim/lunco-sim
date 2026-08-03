within LunCo.Propulsion;

model CombustionChamber
  "Simplified bipropellant chamber and nozzle performance model"
  extends LunCo.Icons.CombustionChamber;

  parameter Real oxidizer_to_fuel_ratio = 2.6;
  parameter Real characteristic_velocity_mps = 1550.0;
  parameter Real throat_area_m2 = 0.0045;
  parameter Real effective_exhaust_velocity_mps = 2940.0;
  parameter Real combustion_efficiency = 0.96;
  parameter Real nominal_flow_kgs = 5.0;
  parameter Real chamber_temperature_full_k = 3300.0;
  parameter Real minimum_flow_kgs = 1.0e-6;
  parameter Real minimum_throat_area_m2 = 1.0e-9;
  parameter Real minimum_mixture_ratio = 1.0e-6;
  parameter Real fuel_energy_j_kg = 4.0e7;

  FluidPort fuel_in "Fuel feed connection";
  FluidPort oxidizer_in "Oxidizer feed connection";
  output Real propellant_flow "Total chamber propellant flow, kg/s";
  output Real mixture_ratio "Oxidizer/fuel mixture ratio";
  output Real mixture_ratio_error "Actual minus requested mixture ratio";
  output Real mixture_efficiency "Efficiency factor from mixture-ratio error";
  output Real chamber_pressure_pa "Estimated chamber pressure, Pa";
  output Real ideal_chamber_pressure_pa "Pressure implied by c-star and throat area, Pa";
  output Real chamber_temperature_k "Estimated combustion temperature, K";
  output Real thrust_n "Generated thrust, N";
  output Real activity "Combustion activity, 0..1";
  output Real heat_release_w "Approximate released chemical power, W";

  output Real fuel_flow_kgs "Fuel mass flow read from the feed connector, kg/s";
  output Real oxidizer_flow_kgs "Oxidizer mass flow read from the feed connector, kg/s";

equation
  fuel_flow_kgs = max(0.0, fuel_in.mass_flow_kgs);
  oxidizer_flow_kgs = max(0.0, oxidizer_in.mass_flow_kgs);
  propellant_flow = max(0.0, fuel_flow_kgs) + max(0.0, oxidizer_flow_kgs);
  mixture_ratio = max(0.0, oxidizer_flow_kgs)
    / max(minimum_flow_kgs, max(0.0, fuel_flow_kgs));
  mixture_ratio_error = mixture_ratio - oxidizer_to_fuel_ratio;
  mixture_efficiency = max(0.0, min(1.0,
    1.0 - abs(mixture_ratio_error)
      / max(minimum_mixture_ratio, oxidizer_to_fuel_ratio)));
  activity = max(0.0, min(1.0, propellant_flow / max(minimum_flow_kgs, nominal_flow_kgs)));
  chamber_pressure_pa = 0.5 * (fuel_in.pressure_pa + oxidizer_in.pressure_pa);
  ideal_chamber_pressure_pa = propellant_flow * characteristic_velocity_mps
    / max(minimum_throat_area_m2, throat_area_m2);
  chamber_temperature_k = chamber_temperature_full_k * activity
    * combustion_efficiency * mixture_efficiency;
  thrust_n = propellant_flow * effective_exhaust_velocity_mps
    * combustion_efficiency * mixture_efficiency;
  heat_release_w = propellant_flow * fuel_energy_j_kg
    * combustion_efficiency * mixture_efficiency;
end CombustionChamber;
