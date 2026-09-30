within LunCo.Propulsion;
model PlumePhotometry "What an exhaust plume is worth as a light source."
  extends LunCo.Icons.Propulsion;

  parameter Integer engine_count(min = 1) = 1
    "Equal nozzles represented by the aggregate propulsion outputs";

  // Emissive geometry in a forward renderer illuminates nothing. A descent burn
  // therefore leaves the regolith directly under the vehicle lit only by the sun,
  // which on an airless body — hard shadows, no atmospheric scatter to hide it —
  // is the most obviously wrong thing in frame. This model derives the light from
  // the same engine and nozzle signals that make the plume, so it cannot become a
  // second, hand-tuned brightness control.
  //
  //   engine outputs + nozzle design + authored pressure threshold ─► this model
  //                                                                  │
  //                    plume length, luminous power, source radius ◄─┘
  //
  // `pressure_threshold_pa` is an explicit presentation assumption: the
  // pressure below which the free jet is no longer a visible plume. It is not a
  // throttle or fuel proxy. The remaining varying quantities are read from the
  // propulsion and nozzle models through USD connections.

  input Real throttle = 0.0
    "Command or combustion activity 0..1; delivered thrust gates the plume";
  input Real thrust_n = 0.0 "Current total cluster thrust magnitude (N)";
  input Real maximum_thrust_n = 1000.0 "Total cluster thrust capability at nominal flow (N)";
  input Real propellant_flow_kgs = 0.0 "Current total cluster propellant flow (kg/s)";
  input Real exhaust_velocity_mps = 0.0
    "Current effective exhaust velocity after mixture losses (m/s)";
  input Real design_exhaust_velocity_mps = 2600.0
    "Nominal effective exhaust velocity before current mixture losses (m/s)";
  input Real nozzle_exit_radius_m = 0.10 "One nozzle's exit radius (m)";
  input Real nozzle_exit_area_m2 = 0.0 "One nozzle's exit area (m2)";
  input Real nozzle_exit_pressure_pa = 0.0 "Nozzle design exit pressure (Pa)";
  input Real chamber_pressure_pa = 0.0 "Current chamber pressure (Pa)";
  input Real design_chamber_pressure_pa = 5500000.0 "Nozzle design chamber pressure (Pa)";
  input Real ambient_pressure_pa = 0.0 "Ambient pressure at the nozzle (Pa)";
  input Real pressure_threshold_pa = 1000.0
    "Visible free-jet pressure floor (Pa)";

  // The shader draws inside a Modelica-derived capacity. Capacity is a render envelope,
  // not a physical length: the physical length below is derived and the shader
  // receives its fraction through `visual_length_fraction`.
  input Real geometry_capacity_m = 0.0 "Optional fixed capacity (m); zero derives capacity from the design plume";

  // The shader's shape law remains shared by the simulation-side light and the
  // rendered plume. `w_max` optionally overrides the derived core radius; axial
  // capacity is supplied separately so the physical length is not hand-tuned.
  input Real w_max = 0.0 "Plume base radius at full throttle (m)";
  input Real width_idle = 0.28
    "Base-radius fraction at zero throttle; width blooms fast, then saturates";
  input Real throttle_exponent = 0.35
    "Perceptual plume response; must match the bound plume shader";

  // Rec.709 luma of the derived colour, with an optional explicit override. The standard luminance
  // weighting, and the reason a green flame of the same RGB magnitude lights a
  // scene far more than a blue one. The colour is LINEAR and un-normalised.
  input Real luminance = 0.0 "Rec.709 luma of the plume's emissive colour";

  // The one authored photometric constant: luminous exitance per unit emissive
  // radiance, in lm/m2. Everything that varies is derived above or below.
  input Real exitance = 44200.0
    "Luminous exitance per unit emissive radiance (lm/m2)";

  input Real r_idle = 0.06 "Source radius at zero throttle (m)";
  input Real r_gain = 0.6 "Additional source radius at full throttle (m)";

  // Fuel and richness classify a presentation palette, not a spectrum solver.
  // Unknown fuel defaults to neutral white. Integer codes are deliberately
  // scalar inputs so the same port contract works through existing Modelica USD.
  input Real fuel_family = 0 "0 unknown, 1 hydrocarbon, 2 hydrogen, 3 hypergolic";
  input Real mixture_mode = 0 "-1 fuel rich, +1 oxidizer rich, 0 infer from O/F";
  input Real mixture_ratio = 2.0 "Actual oxidizer/fuel mass ratio";
  input Real stoichiometric_mixture_ratio = 2.0
    "Estimated chemistry reference O/F; author a known value for the fuel";
  input Real radial_expansion = 1.6 "Estimated visible jet radius / nozzle exit radius";
  input Real core_radius_fraction = 0.65 "Core / outer visible envelope radius";
  output Real envelope_radius_m;
  output Real core_envelope_radius_m;
  output Real envelope_length_m;
  output Real envelope_center_y_m;
  output Real color_r;
  output Real color_g;
  output Real color_b;

  output Real width "Plume base radius at this throttle (m)";
  output Real length "Plume length at this throttle (m)";
  output Real full_throttle_length_m
    "Derived plume length at nominal engine output (m)";
  output Real visual_length_fraction
    "Current visible length divided by the fixed shader envelope";
  output Real render_throttle
    "Delivered-thrust throttle used by the plume material (0..1)";
  output Real area "Lateral surface of one plume cone (m2)";
  output Real intensity "Luminous power per nozzle (lm) — Bevy PointLight.intensity";
  output Real visual_intensity
    "Luminous power using the shader's visible throttle response (lm)";
  output Real radius "Physical source radius (m) — Bevy PointLight.radius";
  output Real visual_radius "Source radius using the shader's visible response (m)";
  output Real momentum_flux_n "Current propellant momentum flux per nozzle (N)";
  output Real exit_dynamic_pressure_pa
    "Current exhaust dynamic pressure per nozzle exit (Pa)";

// USD binds these named model ports; the shared function owns their calculation.
equation
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
    visual_intensity,
    radius,
    visual_radius,
    momentum_flux_n,
    exit_dynamic_pressure_pa} = computePlumePhotometry(
    engine_count,
    throttle,
    thrust_n,
    maximum_thrust_n,
    propellant_flow_kgs,
    exhaust_velocity_mps,
    design_exhaust_velocity_mps,
    nozzle_exit_radius_m,
    nozzle_exit_area_m2,
    nozzle_exit_pressure_pa,
    chamber_pressure_pa,
    design_chamber_pressure_pa,
    ambient_pressure_pa,
    pressure_threshold_pa,
    geometry_capacity_m,
    w_max,
    width_idle,
    throttle_exponent,
    luminance,
    exitance,
    r_idle,
    r_gain,
    fuel_family,
    mixture_mode,
    mixture_ratio,
    stoichiometric_mixture_ratio,
    radial_expansion,
    core_radius_fraction);
end PlumePhotometry;
