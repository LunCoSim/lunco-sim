within LunCo.Propulsion;
model NozzleWallThermal
  "Passive lumped wall-temperature estimate; optical study, not flight calibration"
  extends LunCo.Icons.Propulsion;
  parameter Real heat_capacity_j_k = 750.0;
  parameter Real initial_temperature_k = 293.15;
  parameter Real heated_area_m2 = 0.33;
  parameter Real radiating_area_m2 = 0.66;
  parameter Real emissivity = 0.8;
  parameter Real reference_flow_kgs = 0.8;
  parameter Real reference_film_coefficient_w_m2_k = 120.0;
  parameter Real mount_conductance_w_k = 2.0;
  parameter Real mount_temperature_k = 293.15;
  parameter Real surroundings_temperature_k = 3.0;
  parameter Real recovery_factor = 0.9;
  parameter Real maximum_heating_fraction = 0.03;
  input Real propellant_flow_kgs = 0.0;
  input Real exit_temperature_k = 0.0;
  input Real stagnation_temperature_k = 0.0;
  input Real jet_power_w = 0.0;
  output Real wall_temperature_k;
  output Real render_temperature_k "Explicit GPU material projection of wall temperature";
  output Real render_emissivity "Same wall emissivity used by the thermal balance";
  output Real recovery_temperature_k;
  output Real heating_w;
  output Real radiation_w;
  output Real conduction_w;
  output Real stored_heat_rate_w;
  output Real energy_residual_w;
protected
  model WallExchange "Signed gas, radiation and mount heat at the wall node"
    input Real net_heat_w;
    LunCo.Thermal.HeatPort port;
  equation
    port.Q = -net_heat_w;
  end WallExchange;
  constant Real stefan_boltzmann = 5.670374419e-8;
  Real film_coefficient_w_m2_k;
  LunCo.Thermal.ThermalMass wall(c_th=heat_capacity_j_k,T_init=initial_temperature_k);
  WallExchange exchange(net_heat_w=heating_w-radiation_w-conduction_w);
equation
  assert(heat_capacity_j_k > 0.0 and initial_temperature_k > 0.0,
    "Nozzle wall requires positive heat capacity and initial temperature");
  assert(heated_area_m2 > 0.0 and radiating_area_m2 > 0.0 and reference_flow_kgs > 0.0,
    "Nozzle wall requires positive surface areas and reference flow");
  assert(emissivity >= 0.0 and emissivity <= 1.0 and recovery_factor >= 0.0 and recovery_factor <= 1.0,
    "Nozzle wall emissivity and recovery factor must be in [0,1]");
  assert(reference_film_coefficient_w_m2_k >= 0.0 and mount_conductance_w_k >= 0.0
    and mount_temperature_k > 0.0 and surroundings_temperature_k >= 0.0,
    "Nozzle wall heat-transfer parameters are invalid");
  assert(maximum_heating_fraction >= 0.0 and maximum_heating_fraction <= 0.1,
    "Passive nozzle heating fraction must be in [0,0.1]");
  assert(propellant_flow_kgs >= 0.0 and jet_power_w >= 0.0 and exit_temperature_k >= 0.0
    and stagnation_temperature_k >= exit_temperature_k,
    "Nozzle wall requires nonnegative delivered gas state and T0 >= Te");
  wall_temperature_k = wall.temp_k;
  render_temperature_k = wall_temperature_k;
  render_emissivity = emissivity;
  recovery_temperature_k = exit_temperature_k
    + recovery_factor*(stagnation_temperature_k-exit_temperature_k);
  film_coefficient_w_m2_k = reference_film_coefficient_w_m2_k
    *(propellant_flow_kgs/reference_flow_kgs)^0.8;
  // Never extract more heat than the authored fraction of delivered jet power.
  heating_w = min(max(0.0,film_coefficient_w_m2_k*heated_area_m2
    *(recovery_temperature_k-wall_temperature_k)),maximum_heating_fraction*jet_power_w);
  radiation_w = emissivity*stefan_boltzmann*radiating_area_m2
    *(wall_temperature_k^4-surroundings_temperature_k^4);
  conduction_w = mount_conductance_w_k*(wall_temperature_k-mount_temperature_k);
  connect(exchange.port,wall.port);
  stored_heat_rate_w = wall.heat_flow_w;
  energy_residual_w = stored_heat_rate_w-heating_w+radiation_w+conduction_w;
end NozzleWallThermal;
