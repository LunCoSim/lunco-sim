within LunCo.Propulsion;
model NozzleWallThermal
  "Three wall zones with bounded gas heating, axial conduction and radiative cooling"
  extends LunCo.Icons.Propulsion;
  parameter Real heat_capacity_j_k = 750.0;
  parameter Real initial_temperature_k = 293.15;
  parameter Real heated_area_m2 = 0.33;
  parameter Real radiating_area_m2 = 0.33 "Effective outward radiating area; inner wall faces hot gas";
  parameter Real emissivity = 0.8;
  parameter Real reference_flow_kgs = 0.8;
  parameter Real reference_film_coefficient_w_m2_k = 120.0 "Exit-zone reference coefficient";
  parameter Real throat_capacity_fraction = 0.15;
  parameter Real middle_capacity_fraction = 0.35;
  parameter Real axial_conductance_w_k = 0.5;
  parameter Real mount_conductance_w_k = 2.0;
  parameter Real mount_temperature_k = 293.15;
  parameter Real surroundings_temperature_k = 3.0;
  parameter Real recovery_factor = 0.9;
  parameter Real maximum_heating_fraction = 0.03;
  input Real propellant_flow_kgs = 0.0;
  input Real stagnation_temperature_k = 0.0;
  input Real jet_power_w = 0.0;
  input Real throat_radius_m = 0.0184;
  input Real exit_radius_m = 0.184;
  input Real nozzle_length_m = 0.494;
  input Real nozzle_contour = 0.55;
  input Real gas_gamma = 1.2;
  output Real wall_temperature_k "Heat-capacity-weighted mean; not the hottest wall zone";
  output Real throat_temperature_k;
  output Real middle_temperature_k;
  output Real exit_temperature_k;
  output Real render_throat_temperature_k;
  output Real render_middle_temperature_k;
  output Real render_exit_temperature_k;
  output Real render_nozzle_length_m;
  output Real render_emissivity;
  output Real recovery_temperature_k;
  output Real heating_w;
  output Real radiation_w;
  output Real conduction_w;
  output Real stored_heat_rate_w;
  output Real energy_residual_w;
protected
  function distributeHeating "Nonnegative allocation, including the exact zero-flow boundary"
    input Real raw[3];
    input Real budget;
    output Real delivered[3];
  protected
    Real total;
  algorithm
    total := sum(raw);
    if total <= budget then
      delivered[1] := raw[1];
      delivered[2] := raw[2];
      delivered[3] := raw[3];
    else
      // This branch implies total > budget >= 0, so division is defined.
      delivered[1] := budget*raw[1]/total;
      delivered[2] := budget*raw[2]/total;
      delivered[3] := budget*raw[3]/total;
    end if;
  end distributeHeating;
  model WallExchange
    input Real net_heat_w;
    LunCo.Thermal.HeatPort port;
  equation
    port.Q = -net_heat_w;
  end WallExchange;
  constant Real pi = 3.141592653589793;
  constant Real stefan_boltzmann = 5.670374419e-8;
  constant Real boundaries[4] = {0.0,0.25,0.6,1.0};
  constant Real centres[3] = {0.125,0.425,0.8};
  Real edge_radius[4];
  Real segment_area[3];
  Real area_fraction[3];
  Real zone_temperature[3];
  Real zone_mach[3];
  Real zone_recovery[3];
  Real raw_heating[3];
  Real zone_heating[3];
  Real zone_radiation[3];
  Real heat_01;
  Real heat_12;
  LunCo.Thermal.ThermalMass throat(c_th=heat_capacity_j_k*throat_capacity_fraction,T_init=initial_temperature_k);
  LunCo.Thermal.ThermalMass middle(c_th=heat_capacity_j_k*middle_capacity_fraction,T_init=initial_temperature_k);
  LunCo.Thermal.ThermalMass lip(c_th=heat_capacity_j_k*(1.0-throat_capacity_fraction-middle_capacity_fraction),T_init=initial_temperature_k);
  WallExchange throat_exchange(net_heat_w=zone_heating[1]-zone_radiation[1]-conduction_w-heat_01);
  WallExchange middle_exchange(net_heat_w=zone_heating[2]-zone_radiation[2]+heat_01-heat_12);
  WallExchange lip_exchange(net_heat_w=zone_heating[3]-zone_radiation[3]+heat_12);
equation
  assert(heat_capacity_j_k>0.0 and initial_temperature_k>0.0,
    "Nozzle wall requires positive heat capacity and initial temperature");
  assert(heated_area_m2>0.0 and radiating_area_m2>0.0 and reference_flow_kgs>0.0,
    "Nozzle wall requires positive areas and reference flow");
  assert(throat_capacity_fraction>0.0 and middle_capacity_fraction>0.0
    and throat_capacity_fraction+middle_capacity_fraction<1.0,
    "Each nozzle wall zone requires positive heat capacity");
  assert(throat_radius_m>0.0 and exit_radius_m>throat_radius_m and nozzle_length_m>0.0
    and nozzle_contour>0.0 and gas_gamma>1.0 and gas_gamma<2.0,
    "Nozzle wall requires a positive expanding bell geometry and valid gas gamma");
  assert(emissivity>=0.0 and emissivity<=1.0 and recovery_factor>=0.0 and recovery_factor<=1.0,
    "Nozzle emissivity and recovery factor must be in [0,1]");
  assert(reference_film_coefficient_w_m2_k>=0.0 and axial_conductance_w_k>=0.0
    and mount_conductance_w_k>=0.0 and mount_temperature_k>0.0 and surroundings_temperature_k>=0.0,
    "Nozzle wall heat-transfer parameters are invalid");
  assert(maximum_heating_fraction>=0.0 and maximum_heating_fraction<=0.1,
    "Passive nozzle heating fraction must be in [0,0.1]");
  assert(propellant_flow_kgs>=0.0 and jet_power_w>=0.0 and stagnation_temperature_k>=0.0,
    "Nozzle wall requires nonnegative delivered gas state");
  zone_temperature = {throat.temp_k,middle.temp_k,lip.temp_k};
  for i in 1:4 loop
    edge_radius[i] = throat_radius_m+(exit_radius_m-throat_radius_m)*boundaries[i]^nozzle_contour;
  end for;
  for i in 1:3 loop
    segment_area[i] = pi*(edge_radius[i]+edge_radius[i+1])
      *sqrt((edge_radius[i+1]-edge_radius[i])^2+(nozzle_length_m*(boundaries[i+1]-boundaries[i]))^2);
    area_fraction[i] = segment_area[i]/sum(segment_area);
    zone_mach[i] = supersonicExitMach(((throat_radius_m+(exit_radius_m-throat_radius_m)
      *centres[i]^nozzle_contour)/throat_radius_m)^2,gas_gamma);
    zone_recovery[i] = stagnation_temperature_k*(1.0+recovery_factor*(gas_gamma-1.0)*zone_mach[i]^2/2.0)
      /(1.0+(gas_gamma-1.0)*zone_mach[i]^2/2.0);
    // Bartz area dependence: (Ae/A)^0.9 = (re/r)^1.8. This is a reduced
    // axial distribution, not the full Bartz correlation or a CFD solution.
    raw_heating[i] = max(0.0,reference_film_coefficient_w_m2_k
      *(propellant_flow_kgs/reference_flow_kgs)^0.8
      *(exit_radius_m/(throat_radius_m+(exit_radius_m-throat_radius_m)*centres[i]^nozzle_contour))^1.8
      *heated_area_m2*area_fraction[i]*(zone_recovery[i]-zone_temperature[i]));
    zone_radiation[i] = emissivity*stefan_boltzmann*radiating_area_m2*area_fraction[i]
      *(zone_temperature[i]^4-surroundings_temperature_k^4);
  end for;
  zone_heating = distributeHeating(raw_heating,maximum_heating_fraction*jet_power_w);
  heat_01 = axial_conductance_w_k*(throat.temp_k-middle.temp_k);
  heat_12 = axial_conductance_w_k*(middle.temp_k-lip.temp_k);
  conduction_w = mount_conductance_w_k*(throat.temp_k-mount_temperature_k);
  connect(throat_exchange.port,throat.port);
  connect(middle_exchange.port,middle.port);
  connect(lip_exchange.port,lip.port);
  throat_temperature_k = throat.temp_k;
  middle_temperature_k = middle.temp_k;
  exit_temperature_k = lip.temp_k;
  wall_temperature_k = throat_capacity_fraction*throat.temp_k+middle_capacity_fraction*middle.temp_k
    +(1.0-throat_capacity_fraction-middle_capacity_fraction)*lip.temp_k;
  render_throat_temperature_k = throat_temperature_k;
  render_middle_temperature_k = middle_temperature_k;
  render_exit_temperature_k = exit_temperature_k;
  render_nozzle_length_m = nozzle_length_m;
  render_emissivity = emissivity;
  recovery_temperature_k = zone_recovery[1];
  heating_w = sum(zone_heating);
  radiation_w = sum(zone_radiation);
  stored_heat_rate_w = throat.heat_flow_w+middle.heat_flow_w+lip.heat_flow_w;
  energy_residual_w = stored_heat_rate_w-heating_w+radiation_w+conduction_w;
end NozzleWallThermal;
