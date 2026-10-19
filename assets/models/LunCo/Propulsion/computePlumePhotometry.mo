within LunCo.Propulsion;

function computePlumePhotometry
  "Conservative five-station plume and energy-bounded optical assumptions"
  input Integer engine_count;
  input Real thrust_n;
  input Real maximum_thrust_n;
  input Real propellant_flow_kgs;
  input Real exhaust_velocity_mps;
  input Real nozzle_exit_radius_m;
  input Real nozzle_exit_area_m2;
  input Real nozzle_exit_mach;
  input Real gas_gamma;
  input Real gas_constant_j_kg_k;
  input Real ambient_pressure_pa;
  input Real geometry_capacity_m;
  input Real exit_lip_angle_rad;
  input Real radiative_fraction;
  input Real luminous_efficacy_lm_w;
  input Real fuel_family;
  input Real mixture_mode;
  input Real mixture_ratio;
  input Real stoichiometric_mixture_ratio;
  output Real values[52];
protected
  Real envelope_radius_m;
  Real core_envelope_radius_m;
  Real envelope_length_m;
  Real envelope_center_y_m;
  Real color_r;
  Real color_g;
  Real color_b;
  Real width;
  Real length;
  Real full_throttle_length_m;
  Real visual_length_fraction;
  Real render_throttle;
  Real area;
  Real intensity;
  Real radius;
  Real momentum_flux_n;
  Real exit_dynamic_pressure_pa;
  Real profile_radius_m_0;
  Real profile_radius_m_1;
  Real profile_radius_m_2;
  Real profile_radius_m_3;
  Real profile_radius_m_4;
  Real profile_density_kg_m3_0;
  Real profile_density_kg_m3_1;
  Real profile_density_kg_m3_2;
  Real profile_density_kg_m3_3;
  Real profile_density_kg_m3_4;
  Real profile_temperature_k_0;
  Real profile_temperature_k_1;
  Real profile_temperature_k_2;
  Real profile_temperature_k_3;
  Real profile_temperature_k_4;
  Real profile_speed_mps_0;
  Real profile_speed_mps_1;
  Real profile_speed_mps_2;
  Real profile_speed_mps_3;
  Real profile_speed_mps_4;
  Real profile_emission_w_m3_0;
  Real profile_emission_w_m3_1;
  Real profile_emission_w_m3_2;
  Real profile_emission_w_m3_3;
  Real profile_emission_w_m3_4;
  Real jet_power_w;
  Real radiant_power_w;
  Real mass_residual_kgs;
  Real energy_residual_w;
  Real thrust_residual_n;
  Real exit_pressure_pa;
  Real stagnation_temperature_k;
  Real total_specific_energy_j_kg;
  Real exit_speed_mps;
  Real exit_temperature_k;
  Real p[35];
  Real richness;
  Real fuel_r;
  Real fuel_g;
  Real fuel_b;
  Real delivered_flow;
  Real delivered_thrust;
  Real area_exit;
  constant Real pi=3.141592653589793;
  constant Real minimum_positive=1.0e-12;
algorithm
  assert(engine_count >= 1, "PlumePhotometry requires a positive nozzle count");
  assert(maximum_thrust_n > 0.0 and nozzle_exit_area_m2 >= 0.0,
    "PlumePhotometry requires positive thrust capability and nonnegative optional exit area");
  assert(exhaust_velocity_mps >= 0.0 and (thrust_n <= 0.0 or exhaust_velocity_mps > 0.0),
    "Positive plume thrust requires a positive effective Isp velocity");
  assert(luminous_efficacy_lm_w >= 0.0 and luminous_efficacy_lm_w <= 683.0,
    "Plume luminous efficacy must be in [0,683] lm/W");
  assert(geometry_capacity_m >= 0.0, "Plume observation capacity cannot be negative");
  assert(thrust_n >= 0.0 and propellant_flow_kgs >= 0.0, "Plume inputs are delivered nonnegative magnitudes");
  area_exit := if nozzle_exit_area_m2 > 0.0 then nozzle_exit_area_m2 else pi*nozzle_exit_radius_m^2;
  assert(abs(area_exit-pi*nozzle_exit_radius_m^2) <= 1.0e-5*area_exit,
    "Plume exit area and radius disagree");
  delivered_flow := propellant_flow_kgs/engine_count;
  delivered_thrust := thrust_n/engine_count;
  envelope_length_m := if geometry_capacity_m > 0.0 then geometry_capacity_m else 20.0*nozzle_exit_radius_m;
  envelope_center_y_m := -0.5*envelope_length_m;
  p := computeFreeJetProfile(delivered_flow,delivered_thrust,nozzle_exit_radius_m,
    nozzle_exit_mach,gas_gamma,gas_constant_j_kg_k,ambient_pressure_pa,envelope_length_m,
    exit_lip_angle_rad,radiative_fraction);
  profile_radius_m_0 := p[1];
  profile_radius_m_1 := p[2];
  profile_radius_m_2 := p[3];
  profile_radius_m_3 := p[4];
  profile_radius_m_4 := p[5];
  profile_density_kg_m3_0 := p[6];
  profile_density_kg_m3_1 := p[7];
  profile_density_kg_m3_2 := p[8];
  profile_density_kg_m3_3 := p[9];
  profile_density_kg_m3_4 := p[10];
  profile_temperature_k_0 := p[11];
  profile_temperature_k_1 := p[12];
  profile_temperature_k_2 := p[13];
  profile_temperature_k_3 := p[14];
  profile_temperature_k_4 := p[15];
  profile_speed_mps_0 := p[16];
  profile_speed_mps_1 := p[17];
  profile_speed_mps_2 := p[18];
  profile_speed_mps_3 := p[19];
  profile_speed_mps_4 := p[20];
  profile_emission_w_m3_0 := p[21];
  profile_emission_w_m3_1 := p[22];
  profile_emission_w_m3_2 := p[23];
  profile_emission_w_m3_3 := p[24];
  profile_emission_w_m3_4 := p[25];
  jet_power_w := p[26];
  radiant_power_w := p[27];
  mass_residual_kgs := p[28];
  energy_residual_w := p[29];
  thrust_residual_n := max(abs(p[30]),abs(thrust_n-propellant_flow_kgs*exhaust_velocity_mps)/engine_count);
  exit_pressure_pa := p[31];
  stagnation_temperature_k := p[32];
  total_specific_energy_j_kg := p[33];
  exit_speed_mps := p[34];
  exit_temperature_k := p[35];
  richness := max(-1.0, min(1.0, mixture_mode
    + (1.0 - min(1.0, abs(mixture_mode)))
      * (mixture_ratio / max(minimum_positive, stoichiometric_mixture_ratio) - 1.0)));
  fuel_r := 0.85 * max(0.0, 1.0 - abs(fuel_family))
    + max(0.0, 1.0 - abs(fuel_family - 1.0))
    + 0.35 * max(0.0, 1.0 - abs(fuel_family - 2.0))
    + max(0.0, 1.0 - abs(fuel_family - 3.0));
  fuel_g := 0.90 * max(0.0, 1.0 - abs(fuel_family))
    + 0.65 * max(0.0, 1.0 - abs(fuel_family - 1.0))
    + 0.60 * max(0.0, 1.0 - abs(fuel_family - 2.0))
    + 0.75 * max(0.0, 1.0 - abs(fuel_family - 3.0));
  fuel_b := max(0.0, 1.0 - abs(fuel_family))
    + 0.30 * max(0.0, 1.0 - abs(fuel_family - 1.0))
    + max(0.0, 1.0 - abs(fuel_family - 2.0))
    + 0.50 * max(0.0, 1.0 - abs(fuel_family - 3.0));
  color_r := max(0.0, min(1.0, fuel_r - 0.15 * richness));
  color_g := max(0.0, min(1.0, fuel_g + 0.15 * richness));
  color_b := max(0.0, min(1.0, fuel_b + 0.25 * richness));
  envelope_radius_m := profile_radius_m_4;
  core_envelope_radius_m := envelope_radius_m;
  width := nozzle_exit_radius_m;
  length := if radiant_power_w > 0.0 then envelope_length_m else 0.0;
  full_throttle_length_m := envelope_length_m;
  visual_length_fraction := if radiant_power_w > 0.0 then 1.0 else 0.0;
  render_throttle := if delivered_flow > 0.0 and exhaust_velocity_mps > 0.0 then
    min(1.0,max(0.0,thrust_n)/max(minimum_positive,maximum_thrust_n)) else 0.0;
  area := pi*(nozzle_exit_radius_m+envelope_radius_m)
    *sqrt(envelope_length_m^2+(envelope_radius_m-nozzle_exit_radius_m)^2);
  intensity := luminous_efficacy_lm_w*radiant_power_w;
  radius := nozzle_exit_radius_m;
  momentum_flux_n := delivered_flow*exit_speed_mps;
  exit_dynamic_pressure_pa := 0.5*momentum_flux_n/area_exit;
  values := {envelope_radius_m,
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
    exit_temperature_k};
end computePlumePhotometry;
