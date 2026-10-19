within LunCo.Propulsion;

model RCSJet
  "One scalar RCS nozzle model; USD owns its physical mount"
  extends LunCo.Icons.RCSJet;

  parameter Real f_nom_n = 2500.0 "Nominal nozzle force (N)";
  parameter Real isp_sec = 220.0 "Specific impulse (s)";
  parameter Real g0 = 9.80665 "Standard gravity acceleration (m/s2)";
  parameter Real minimum_isp_g0 = 1.0e-6
    "Smallest specific-impulse/gravity product used for flow";
  // Nozzle geometry sets the supersonic exit state and forward expansion.
  parameter Real plume_width_m = 0.28 "Effective exit radius (m)";
  parameter Real ambient_pressure_pa = 0.0 "Ambient pressure at the nozzle (Pa)";
  parameter Real nozzle_throat_radius_m = 0.028;
  parameter Real gas_gamma = 1.2;
  parameter Real gas_constant_j_kg_k = 300.0;
  parameter Real nozzle_length_m = 0.5;
  parameter Real nozzle_contour = 0.55;
  parameter Real radiative_fraction = 0.00005;
  parameter Real luminous_efficacy_lm_w = 200.0;
  parameter Real geometry_capacity_m = 0.0;

  input Real valve_opening "RCS valve opening, 0..1";
  output Real thrust_n "Nozzle thrust magnitude (N)";
  output Real mass_flow_kgs "Propellant flow (kg/s)";
  output Real activity "Normalized valve activity, 0..1";
  output Real light_intensity "RCS plume luminous power (lm)";
  output Real light_radius "RCS plume source radius (m)";
  output Real full_throttle_length_m
    "Finite rendering observation horizon (m), not a vacuum plume end";
  output Real visual_length_fraction
    "Current visible length divided by the fixed shader envelope";
  output Real exit_dynamic_pressure_pa
    "Current exhaust dynamic pressure at the effective exit (Pa)";

  input Real available_fuel_mass_kg = 1.0 "Feed reservoir availability (kg); connect the owning tank";
  input Real available_oxidizer_mass_kg = 1.0 "Oxidizer reservoir availability (kg)";
  parameter Real availability_transition_mass_kg = 0.01 "Continuous feed starvation transition (kg)";
  parameter Real fuel_family = 0 "PlumePhotometry fuel classification";
  parameter Real mixture_ratio = 2.6 "Assumed feed O/F ratio; actual split is owned by the bank";
  parameter Real stoichiometric_mixture_ratio = 2.6 "Estimated chemistry reference O/F";
  output Real envelope_radius_m;
  output Real core_envelope_radius_m;
  output Real envelope_length_m;
  output Real envelope_center_y_m;
  output Real color_r;
  output Real color_g;
  output Real color_b;
  output Real width;
  output Real length;
  output Real render_throttle;
  output Real area;
  output Real momentum_flux_n;
  output Real profile_radius_m_0;
  output Real profile_radius_m_1;
  output Real profile_radius_m_2;
  output Real profile_radius_m_3;
  output Real profile_radius_m_4;
  output Real profile_density_kg_m3_0;
  output Real profile_density_kg_m3_1;
  output Real profile_density_kg_m3_2;
  output Real profile_density_kg_m3_3;
  output Real profile_density_kg_m3_4;
  output Real profile_temperature_k_0;
  output Real profile_temperature_k_1;
  output Real profile_temperature_k_2;
  output Real profile_temperature_k_3;
  output Real profile_temperature_k_4;
  output Real profile_speed_mps_0;
  output Real profile_speed_mps_1;
  output Real profile_speed_mps_2;
  output Real profile_speed_mps_3;
  output Real profile_speed_mps_4;
  output Real profile_emission_w_m3_0;
  output Real profile_emission_w_m3_1;
  output Real profile_emission_w_m3_2;
  output Real profile_emission_w_m3_3;
  output Real profile_emission_w_m3_4;
  output Real jet_power_w;
  output Real radiant_power_w;
  output Real mass_residual_kgs;
  output Real energy_residual_w;
  output Real thrust_residual_n;
  output Real exit_pressure_pa;
  output Real stagnation_temperature_k;
  output Real total_specific_energy_j_kg;
  output Real exit_speed_mps;
  output Real exit_temperature_k;
  output Real flow_phase_sin;
  output Real flow_phase_cos;
  output Real transported_distance_m(start=0.0,fixed=true);
  RCSThruster thruster(f_nom_n=f_nom_n, isp_sec=isp_sec, g0=g0,
    minimum_isp_g0=minimum_isp_g0);

  Real feed_availability;
  Real exhaust_velocity_mps;
  output Real luminous_efficacy_out_lm_w;
equation
  assert(nozzle_length_m > 0.0 and nozzle_contour > 0.0, "RCS nozzle contour requires positive length/exponent");
  luminous_efficacy_out_lm_w = luminous_efficacy_lm_w;
  feed_availability = min(max(0.0, min(1.0, available_fuel_mass_kg
      / max(minimum_isp_g0, availability_transition_mass_kg))),
    max(0.0, min(1.0, available_oxidizer_mass_kg
      / max(minimum_isp_g0, availability_transition_mass_kg))));
  thruster.valve_opening = valve_opening * feed_availability;
  thrust_n = thruster.thrust_n;
  mass_flow_kgs = thruster.mass_flow_kgs;
  activity = max(0.0, min(1.0, thrust_n / max(minimum_isp_g0, f_nom_n)));
  exhaust_velocity_mps = max(0.0, isp_sec) * max(0.0, g0);
  {envelope_radius_m,
    core_envelope_radius_m,
    envelope_length_m,
    envelope_center_y_m,
    color_r,
    color_g,
    color_b,
    width,
    length,
    full_throttle_length_m,
    visual_length_fraction,
    render_throttle,
    area,
    light_intensity,
    light_radius,
    momentum_flux_n,
    exit_dynamic_pressure_pa,
    profile_radius_m_0,
    profile_radius_m_1,
    profile_radius_m_2,
    profile_radius_m_3,
    profile_radius_m_4,
    profile_density_kg_m3_0,
    profile_density_kg_m3_1,
    profile_density_kg_m3_2,
    profile_density_kg_m3_3,
    profile_density_kg_m3_4,
    profile_temperature_k_0,
    profile_temperature_k_1,
    profile_temperature_k_2,
    profile_temperature_k_3,
    profile_temperature_k_4,
    profile_speed_mps_0,
    profile_speed_mps_1,
    profile_speed_mps_2,
    profile_speed_mps_3,
    profile_speed_mps_4,
    profile_emission_w_m3_0,
    profile_emission_w_m3_1,
    profile_emission_w_m3_2,
    profile_emission_w_m3_3,
    profile_emission_w_m3_4,
    jet_power_w,
    radiant_power_w,
    mass_residual_kgs,
    energy_residual_w,
    thrust_residual_n,
    exit_pressure_pa,
    stagnation_temperature_k,
    total_specific_energy_j_kg,
    exit_speed_mps,
    exit_temperature_k} = computePlumePhotometry(
    engine_count=1,
    thrust_n=thrust_n,
    maximum_thrust_n=f_nom_n,
    propellant_flow_kgs=mass_flow_kgs,
    exhaust_velocity_mps=exhaust_velocity_mps,
    nozzle_exit_radius_m=plume_width_m,
    nozzle_exit_area_m2=0.0,
    nozzle_exit_mach=supersonicExitMach((plume_width_m/nozzle_throat_radius_m)^2,gas_gamma),
    gas_gamma=gas_gamma,
    gas_constant_j_kg_k=gas_constant_j_kg_k,
    ambient_pressure_pa=ambient_pressure_pa,
    geometry_capacity_m=geometry_capacity_m,
    exit_lip_angle_rad=atan((plume_width_m-nozzle_throat_radius_m)*nozzle_contour/nozzle_length_m),
    radiative_fraction=radiative_fraction,
    luminous_efficacy_lm_w=luminous_efficacy_lm_w,
    fuel_family=fuel_family,
    mixture_mode=0.0,
    mixture_ratio=mixture_ratio,
    stoichiometric_mixture_ratio=stoichiometric_mixture_ratio);
  der(transported_distance_m) = exit_speed_mps;
  flow_phase_sin = sin(transported_distance_m/(1024.0*plume_width_m)*6.283185307179586);
  flow_phase_cos = cos(transported_distance_m/(1024.0*plume_width_m)*6.283185307179586);
end RCSJet;
