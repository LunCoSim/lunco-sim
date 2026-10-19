within LunCo.Propulsion;
model PlumePhotometry
  "Supersonic free-jet stations and energy-bounded radiative estimate"
  extends LunCo.Icons.Propulsion;
  parameter Integer engine_count = 1;
  input Real thrust_n = 0.0;
  input Real maximum_thrust_n = 1000.0;
  input Real propellant_flow_kgs = 0.0;
  input Real exhaust_velocity_mps = 0.0;
  input Real nozzle_exit_radius_m = 0.10;
  input Real nozzle_exit_area_m2 = 0.0;
  input Real nozzle_exit_mach = 3.8;
  input Real gas_gamma = 1.2;
  input Real gas_constant_j_kg_k = 300.0;
  input Real ambient_pressure_pa = 0.0;
  input Real geometry_capacity_m = 0.0;
  input Real exit_lip_angle_rad = 0.08;
  input Real radiative_fraction = 0.0002;
  input Real luminous_efficacy_lm_w = 200.0;
  input Real fuel_family = 0.0;
  input Real mixture_mode = 0.0;
  input Real mixture_ratio = 2.0;
  input Real stoichiometric_mixture_ratio = 2.0;
  output Real envelope_radius_m;
  output Real core_envelope_radius_m;
  output Real envelope_length_m;
  output Real envelope_center_y_m;
  output Real color_r;
  output Real color_g;
  output Real color_b;
  output Real width;
  output Real length;
  output Real full_throttle_length_m;
  output Real visual_length_fraction;
  output Real render_throttle;
  output Real area;
  output Real intensity;
  output Real radius;
  output Real momentum_flux_n;
  output Real exit_dynamic_pressure_pa;
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
  output Real luminous_efficacy_out_lm_w;
equation
  luminous_efficacy_out_lm_w = luminous_efficacy_lm_w;
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
    intensity,
    radius,
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
    engine_count,
    thrust_n,
    maximum_thrust_n,
    propellant_flow_kgs,
    exhaust_velocity_mps,
    nozzle_exit_radius_m,
    nozzle_exit_area_m2,
    nozzle_exit_mach,
    gas_gamma,
    gas_constant_j_kg_k,
    ambient_pressure_pa,
    geometry_capacity_m,
    exit_lip_angle_rad,
    radiative_fraction,
    luminous_efficacy_lm_w,
    fuel_family,
    mixture_mode,
    mixture_ratio,
    stoichiometric_mixture_ratio);
  der(transported_distance_m) = exit_speed_mps;
  flow_phase_sin = sin(transported_distance_m/(1024.0*nozzle_exit_radius_m)*6.283185307179586);
  flow_phase_cos = cos(transported_distance_m/(1024.0*nozzle_exit_radius_m)*6.283185307179586);
end PlumePhotometry;
