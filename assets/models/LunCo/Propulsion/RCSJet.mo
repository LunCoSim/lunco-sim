within LunCo.Propulsion;

model RCSJet
  "One scalar RCS nozzle model; USD owns its physical mount"
  extends LunCo.Icons.RCSJet;

  parameter Real f_nom_n = 2500.0 "Nominal nozzle force (N)";
  parameter Real isp_sec = 220.0 "Specific impulse (s)";
  parameter Real g0 = 9.80665 "Standard gravity acceleration (m/s2)";
  parameter Real minimum_isp_g0 = 1.0e-6
    "Smallest specific-impulse/gravity product used for flow";
  parameter Real plume_width_m = 0.18 "Full-throttle plume radius (m)";
  parameter Real plume_length_m = 0.72 "Full-throttle plume length (m)";
  parameter Real plume_luminance = 5.475
    "Rec.709 luma of the RCS plume's emissive colour";
  parameter Real plume_exitance = 44200.0
    "Luminous exitance per unit emissive radiance (lm/m2)";
  parameter Real plume_width_idle = 0.28
    "Zero-valve plume width fraction, matching the shader";
  parameter Real plume_throttle_exponent = 0.35
    "Visible throttle response exponent, matching the shader";
  parameter Real plume_radius_idle = 0.06
    "Visible plume source radius at zero valve opening (m)";
  parameter Real plume_radius_gain = 0.6
    "Visible plume source-radius growth at full valve opening (m)";

  input Real valve_opening "RCS valve opening, 0..1";
  output Real thrust_n "Nozzle thrust magnitude (N)";
  output Real mass_flow_kgs "Propellant flow (kg/s)";
  output Real activity "Normalized valve activity, 0..1";
  output Real light_intensity "RCS plume luminous power (lm)";
  output Real light_radius "RCS plume source radius (m)";

  RCSThruster thruster(
    f_nom_n = f_nom_n,
    isp_sec = isp_sec,
    g0 = g0,
    minimum_isp_g0 = minimum_isp_g0);
  PlumePhotometry photometry(
    w_max = plume_width_m,
    l_max = plume_length_m,
    luminance = plume_luminance,
    exitance = plume_exitance);

equation
  thruster.valve_opening = valve_opening;
  thrust_n = thruster.thrust_n;
  mass_flow_kgs = thruster.mass_flow_kgs;
  activity = max(0.0, min(1.0, valve_opening));
  photometry.throttle = activity;
  photometry.width_idle = plume_width_idle;
  photometry.throttle_exponent = plume_throttle_exponent;
  photometry.r_idle = plume_radius_idle;
  photometry.r_gain = plume_radius_gain;
  light_intensity = photometry.visual_intensity;
  light_radius = photometry.visual_radius;
end RCSJet;
