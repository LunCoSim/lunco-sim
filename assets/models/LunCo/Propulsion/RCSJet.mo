within LunCo.Propulsion;

model RCSJet
  "One scalar RCS nozzle model; USD owns its physical mount"
  extends LunCo.Icons.RCSJet;

  parameter Real f_nom_n = 2500.0 "Nominal nozzle force (N)";
  parameter Real isp_sec = 220.0 "Specific impulse (s)";
  parameter Real g0 = 9.80665 "Standard gravity acceleration (m/s2)";
  parameter Real minimum_isp_g0 = 1.0e-6
    "Smallest specific-impulse/gravity product used for flow";
  // The plume width is also the effective exit radius for this compact jet.
  // Shared PlumePhotometry derives length from delivered momentum and pressure.
  parameter Real plume_width_m = 0.28 "Effective exit radius (m)";
  parameter Real geometry_capacity_m = 0.0 "Optional shader envelope length; zero derives from physics (m)";
  parameter Real pressure_threshold_pa = 1000.0
    "Visible free-jet pressure floor (Pa)";
  parameter Real plume_luminance = 0.0
    "Optional Rec.709 luma; zero derives from fuel colour";
  parameter Real plume_exitance = 44200.0
    "Luminous exitance per unit emissive radiance (lm/m2)";
  parameter Real plume_width_idle = 0.28
    "Zero-valve plume width fraction, matching the shader";
  parameter Real plume_throttle_exponent = 0.35
    "Visible throttle response exponent, matching the shader";
  parameter Real plume_radius_idle = 0.06
    "Visible plume source radius at zero valve opening (m)";
  parameter Real plume_radius_gain = 0.8
    "Visible plume source-radius growth at full valve opening (m)";

  input Real valve_opening "RCS valve opening, 0..1";
  output Real thrust_n "Nozzle thrust magnitude (N)";
  output Real mass_flow_kgs "Propellant flow (kg/s)";
  output Real activity "Normalized valve activity, 0..1";
  output Real light_intensity "RCS plume luminous power (lm)";
  output Real light_radius "RCS plume source radius (m)";
  output Real full_throttle_length_m
    "Derived plume length at nominal nozzle force (m)";
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

  RCSThruster thruster(f_nom_n=f_nom_n, isp_sec=isp_sec, g0=g0,
    minimum_isp_g0=minimum_isp_g0);
  PlumePhotometry photometry(
    fuel_family=fuel_family, mixture_ratio=mixture_ratio,
    stoichiometric_mixture_ratio=stoichiometric_mixture_ratio,
    nozzle_exit_radius_m=plume_width_m, geometry_capacity_m=geometry_capacity_m,
    pressure_threshold_pa=pressure_threshold_pa, luminance=plume_luminance,
    exitance=plume_exitance, width_idle=plume_width_idle,
    throttle_exponent=plume_throttle_exponent, r_idle=plume_radius_idle,
    r_gain=plume_radius_gain);
  Real feed_availability;
  Real exhaust_velocity_mps;
equation
  feed_availability = min(max(0.0, min(1.0, available_fuel_mass_kg
      / max(minimum_isp_g0, availability_transition_mass_kg))),
    max(0.0, min(1.0, available_oxidizer_mass_kg
      / max(minimum_isp_g0, availability_transition_mass_kg))));
  thruster.valve_opening = valve_opening * feed_availability;
  thrust_n = thruster.thrust_n;
  mass_flow_kgs = thruster.mass_flow_kgs;
  activity = max(0.0, min(1.0, thrust_n / max(minimum_isp_g0, f_nom_n)));
  exhaust_velocity_mps = max(0.0, isp_sec) * max(0.0, g0);
  photometry.throttle = activity;
  photometry.thrust_n = thrust_n;
  photometry.maximum_thrust_n = f_nom_n;
  photometry.propellant_flow_kgs = mass_flow_kgs;
  photometry.exhaust_velocity_mps = exhaust_velocity_mps;
  photometry.design_exhaust_velocity_mps = exhaust_velocity_mps;
  light_intensity = photometry.intensity;
  light_radius = photometry.radius;
  full_throttle_length_m = photometry.full_throttle_length_m;
  visual_length_fraction = photometry.visual_length_fraction;
  exit_dynamic_pressure_pa = photometry.exit_dynamic_pressure_pa;
  envelope_radius_m = photometry.envelope_radius_m;
  core_envelope_radius_m = photometry.core_envelope_radius_m;
  envelope_length_m = photometry.envelope_length_m;
  envelope_center_y_m = photometry.envelope_center_y_m;
  color_r = photometry.color_r;
  color_g = photometry.color_g;
  color_b = photometry.color_b;
end RCSJet;
