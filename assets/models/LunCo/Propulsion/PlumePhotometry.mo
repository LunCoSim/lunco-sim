within LunCo.Propulsion;
model PlumePhotometry "What an exhaust plume is worth as a light source."
  extends LunCo.Icons.Propulsion;
  constant Real pi = 3.141592653589793 "Circle constant";
  constant Real minimum_positive = 1.0e-9 "Numerical floor for denominators";

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
  // `pressure_threshold_pa` is the one explicit presentation assumption: the
  // pressure below which the free jet is no longer a visible plume. It is not a
  // throttle or fuel proxy. The remaining varying quantities are read from the
  // propulsion and nozzle models through USD connections.

  input Real throttle = 0.0
    "Commanded engine valve 0..1 — wired from the flight-control output";
  input Real thrust_n = 0.0 "Current engine thrust magnitude (N)";
  input Real maximum_thrust_n = 0.0 "Engine thrust capability at nominal flow (N)";
  input Real propellant_flow_kgs = 0.0 "Current total propellant flow (kg/s)";
  input Real exhaust_velocity_mps = 0.0
    "Current effective exhaust velocity after mixture losses (m/s)";
  input Real design_exhaust_velocity_mps = 0.0
    "Nominal effective exhaust velocity before current mixture losses (m/s)";
  input Real nozzle_exit_radius_m = 0.0 "Nozzle exit radius (m)";
  input Real nozzle_exit_area_m2 = 0.0 "Nozzle exit area (m2)";
  input Real nozzle_exit_pressure_pa = 0.0 "Nozzle design exit pressure (Pa)";
  input Real chamber_pressure_pa = 0.0 "Current chamber pressure (Pa)";
  input Real design_chamber_pressure_pa = 0.0 "Nozzle design chamber pressure (Pa)";
  input Real ambient_pressure_pa = 0.0 "Ambient pressure at the nozzle (Pa)";
  input Real pressure_threshold_pa = 1000.0
    "Visible free-jet pressure floor (Pa)";

  // The shader draws inside this fixed capacity. Capacity is a render envelope,
  // not a physical length: the physical length below is derived and the shader
  // receives its fraction through `visual_length_fraction`.
  input Real geometry_capacity_m = 4.0 "Fixed shader envelope length (m)";

  // The shader's shape law remains shared by the simulation-side light and the
  // rendered plume. `w_max` is the core cone's authored base radius; axial
  // capacity is supplied separately so the physical length is not hand-tuned.
  input Real w_max = 0.5 "Plume base radius at full throttle (m)";
  input Real width_idle = 0.28
    "Base-radius fraction at zero throttle; width blooms fast, then saturates";
  input Real throttle_exponent = 0.35
    "Perceptual plume response; must match the bound plume shader";

  // Rec.709 luma of the plume's authored colour. The standard luminance
  // weighting, and the reason a green flame of the same RGB magnitude lights a
  // scene far more than a blue one. The colour is LINEAR and un-normalised.
  input Real luminance = 12.903 "Rec.709 luma of the plume's emissive colour";

  // The one authored photometric constant: luminous exitance per unit emissive
  // radiance, in lm/m2. Everything that varies is derived above or below.
  input Real exitance = 44200.0
    "Luminous exitance per unit emissive radiance (lm/m2)";

  input Real r_idle = 0.06 "Source radius at zero throttle (m)";
  input Real r_gain = 0.6 "Additional source radius at full throttle (m)";

  output Real width "Plume base radius at this throttle (m)";
  output Real length "Plume length at this throttle (m)";
  output Real full_throttle_length_m
    "Derived plume length at nominal engine output (m)";
  output Real visual_length_fraction
    "Current visible length divided by the fixed shader envelope";
  output Real render_throttle
    "Delivered-thrust throttle used by the plume material (0..1)";
  output Real area "Lateral surface of the plume cone (m2)";
  output Real intensity "Luminous power (lm) — Bevy PointLight.intensity";
  output Real visual_intensity
    "Luminous power using the shader's visible throttle response (lm)";
  output Real radius "Physical source radius (m) — Bevy PointLight.radius";
  output Real visual_radius "Source radius using the shader's visible response (m)";
  output Real momentum_flux_n "Current propellant momentum flux (N)";
  output Real exit_dynamic_pressure_pa
    "Current exhaust dynamic pressure at the nozzle exit (Pa)";

  Real t "Delivered throttle, bounded by command activity and thrust";
  Real visual_t "Shader-matched visible throttle response";
  Real thrust_fraction "Delivered thrust divided by nominal capability";
  Real design_mass_flow_kgs "Nominal mass flow inferred from thrust capability";
  Real design_exit_dynamic_pressure_pa "Nominal dynamic pressure at the exit";
  Real pressure_margin_pa "Design exit pressure above ambient";
  Real available_chamber_pressure_pa "Current/design chamber pressure envelope";
  Real chamber_pressure_factor "Vacuum-to-ambient pressure availability factor";
  Real plume_pressure_pa "Pressure scale used for the free-jet length basis";

equation
  thrust_fraction = min(1.0, max(0.0, thrust_n)
    / max(minimum_positive, maximum_thrust_n));
  // A valve can have a short authored spool tail after a zero command. The
  // rendered plume follows delivered thrust, so zero thrust removes the light
  // and shader signal on this same equation path instead of leaving a ghost.
  t = min(min(1.0, max(0.0, throttle)), thrust_fraction);
  visual_t = max(0.0, t) ^ max(0.1, min(1.0, throttle_exponent));
  render_throttle = t;

  // A nominal mass flow is not copied from USD: it comes from the engine's
  // published thrust capability and its published design exhaust velocity. The
  // resulting exit dynamic pressure is compared with the authored exit-plane
  // pressure. In vacuum the stronger term wins; with ambient pressure the
  // pressure margin and chamber factor reduce the visible free-jet basis.
  design_mass_flow_kgs = max(0.0, maximum_thrust_n)
    / max(minimum_positive, design_exhaust_velocity_mps);
  design_exit_dynamic_pressure_pa = 0.5 * design_mass_flow_kgs
    * max(0.0, design_exhaust_velocity_mps)
    / max(minimum_positive, nozzle_exit_area_m2);
  pressure_margin_pa = max(0.0, nozzle_exit_pressure_pa - ambient_pressure_pa);
  available_chamber_pressure_pa = max(0.0,
    max(chamber_pressure_pa, design_chamber_pressure_pa));
  chamber_pressure_factor = max(0.0, min(1.0,
    (available_chamber_pressure_pa - ambient_pressure_pa)
      / max(minimum_positive, available_chamber_pressure_pa)));
  plume_pressure_pa = max(pressure_margin_pa, design_exit_dynamic_pressure_pa)
    * chamber_pressure_factor;
  full_throttle_length_m = max(0.0, nozzle_exit_radius_m)
    * sqrt(max(0.0, plume_pressure_pa)
      / max(minimum_positive, pressure_threshold_pa));

  // The shader receives the derived metric, not a second throttle-shaped length
  // law. Its fixed cone is deliberately a capacity envelope; clamping here
  // keeps an unexpected design point from addressing outside that envelope.
  length = visual_t * full_throttle_length_m;
  visual_length_fraction = min(1.0, max(0.0, length)
    / max(minimum_positive, geometry_capacity_m));
  width = (width_idle + (1.0 - width_idle) * visual_t) * w_max;

  // Use both sides of the authored signal: thrust is the delivered force, while
  // flow*velocity is the independently observable momentum estimate. The lower
  // value is the conservative exit-load estimate when mixture efficiency is
  // still settling during spool-up.
  momentum_flux_n = min(max(0.0, thrust_n),
    max(0.0, propellant_flow_kgs) * max(0.0, exhaust_velocity_mps));
  exit_dynamic_pressure_pa = 0.5 * momentum_flux_n
    / max(minimum_positive, nozzle_exit_area_m2);

  // The plume radiates from its flank, so the emitting surface is the cone's
  // lateral area — not its base, not its volume: A = pi*r*sqrt(r2+h2).
  area = pi * width * sqrt(width ^ 2 + length ^ 2);

  // The endpoint is exact: a dead engine emits no light even though the cone's
  // idle width gives its bounded geometry a non-zero lateral area.
  intensity = t * exitance * luminance * area;
  visual_intensity = visual_t * exitance * luminance * area;
  radius = r_idle + t * r_gain;
  visual_radius = r_idle + visual_t * r_gain;
end PlumePhotometry;
