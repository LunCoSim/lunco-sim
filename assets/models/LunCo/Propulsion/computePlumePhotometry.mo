within LunCo.Propulsion;

function computePlumePhotometry
  "Stateless nozzle plume geometry, colour, and luminous power"
  input Integer engine_count;
  input Real throttle
    "Command or combustion activity 0..1; delivered thrust gates the plume";
  input Real thrust_n "Current total cluster thrust magnitude (N)";
  input Real maximum_thrust_n "Total cluster thrust capability at nominal flow (N)";
  input Real propellant_flow_kgs "Current total cluster propellant flow (kg/s)";
  input Real exhaust_velocity_mps
    "Current effective exhaust velocity after mixture losses (m/s)";
  input Real design_exhaust_velocity_mps
    "Nominal effective exhaust velocity before current mixture losses (m/s)";
  input Real nozzle_exit_radius_m "One nozzle's exit radius (m)";
  input Real nozzle_exit_area_m2 "One nozzle's exit area (m2)";
  input Real nozzle_exit_pressure_pa "Nozzle design exit pressure (Pa)";
  input Real chamber_pressure_pa "Current chamber pressure (Pa)";
  input Real design_chamber_pressure_pa "Nozzle design chamber pressure (Pa)";
  input Real ambient_pressure_pa "Ambient pressure at the nozzle (Pa)";
  input Real pressure_threshold_pa
    "Visible free-jet pressure floor (Pa)";
  input Real geometry_capacity_m "Optional fixed capacity (m); zero derives capacity from the design plume";
  input Real w_max "Plume base radius at full throttle (m)";
  input Real width_idle
    "Base-radius fraction at zero throttle; width blooms fast, then saturates";
  input Real throttle_exponent
    "Perceptual plume response; must match the bound plume shader";
  input Real luminance "Rec.709 luma of the plume's emissive colour";
  input Real exitance
    "Luminous exitance per unit emissive radiance (lm/m2)";
  input Real r_idle "Source radius at zero throttle (m)";
  input Real r_gain "Additional source radius at full throttle (m)";
  input Real fuel_family "0 unknown, 1 hydrocarbon, 2 hydrogen, 3 hypergolic";
  input Real mixture_mode "-1 fuel rich, +1 oxidizer rich, 0 infer from O/F";
  input Real mixture_ratio "Actual oxidizer/fuel mass ratio";
  input Real stoichiometric_mixture_ratio
    "Estimated chemistry reference O/F; author a known value for the fuel";
  input Real radial_expansion "Estimated visible jet radius / nozzle exit radius";
  input Real core_radius_fraction "Core / outer visible envelope radius";
  output Real values[19]
    "Named PlumePhotometry outputs in declaration order";
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
  Real visual_intensity;
  Real radius;
  Real visual_radius;
  Real momentum_flux_n;
  Real exit_dynamic_pressure_pa;
  constant Real pi = 3.141592653589793;
  constant Real minimum_positive = 1.0e-9;
  Real richness;
  Real fuel_r;
  Real fuel_g;
  Real fuel_b;
  Real color_luminance;
  Real exit_area_m2;
  Real t "Delivered throttle, bounded by command activity and thrust";
  Real visual_t "Shader-matched visible throttle response";
  Real thrust_fraction "Conservative delivered jet momentum divided by nominal capability";
  Real design_mass_flow_kgs "Nominal mass flow inferred from thrust capability";
  Real design_exit_dynamic_pressure_pa "Nominal dynamic pressure at the exit";
  Real pressure_margin_pa "Design exit pressure above ambient";
  Real available_chamber_pressure_pa "Current/design chamber pressure envelope";
  Real chamber_pressure_factor "Vacuum-to-ambient pressure availability factor";
  Real plume_pressure_pa "Pressure scale used for the free-jet length basis";
algorithm
  // These RGB anchors and interpolation are explicit visualization defaults.
  // Fuel-rich hydrocarbon exhaust is warmer; hydrogen is faint/blue; unknown
  // and oxidizer-rich exhaust are pale. Pressure, species and exposure also
  // affect observed color and are not spectrally modeled here.
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
  color_luminance := max(0.0, luminance)
    + (1.0 - min(1.0, max(0.0, luminance) / minimum_positive))
      * 16.0 * (0.2126 * color_r + 0.7152 * color_g + 0.0722 * color_b);
  exit_area_m2 := max(0.0, nozzle_exit_area_m2)
    + (1.0 - min(1.0, max(0.0, nozzle_exit_area_m2) / minimum_positive))
      * pi * max(minimum_positive, nozzle_exit_radius_m) ^ 2;
  envelope_radius_m := max(0.0, nozzle_exit_radius_m) * max(1.0, radial_expansion);
  core_envelope_radius_m := envelope_radius_m * max(0.01, min(1.0, core_radius_fraction));
  // A nominal mass flow is not copied from USD: it comes from the cluster's
  // published thrust capability and design exhaust velocity, then is divided
  // among equal nozzles. The resulting per-nozzle exit dynamic pressure is
  // compared with the authored exit-plane pressure. In vacuum the stronger term
  // wins; with ambient pressure the pressure margin and chamber factor reduce
  // the visible free-jet basis.
  design_mass_flow_kgs := max(0.0, maximum_thrust_n)
    / (max(1, engine_count) * max(minimum_positive, design_exhaust_velocity_mps));
  design_exit_dynamic_pressure_pa := 0.5 * design_mass_flow_kgs
    * max(0.0, design_exhaust_velocity_mps)
    / max(minimum_positive, exit_area_m2);
  pressure_margin_pa := max(0.0, nozzle_exit_pressure_pa - ambient_pressure_pa);
  available_chamber_pressure_pa := max(0.0,
    max(chamber_pressure_pa, design_chamber_pressure_pa));
  chamber_pressure_factor := max(0.0, min(1.0,
    (available_chamber_pressure_pa - ambient_pressure_pa)
      / max(minimum_positive, available_chamber_pressure_pa)));
  plume_pressure_pa := max(pressure_margin_pa, design_exit_dynamic_pressure_pa)
    * chamber_pressure_factor;
  full_throttle_length_m := max(0.0, nozzle_exit_radius_m)
    * sqrt(max(0.0, plume_pressure_pa)
      / max(minimum_positive, pressure_threshold_pa));
  envelope_length_m := max(0.0, geometry_capacity_m)
    + (1.0 - min(1.0, max(0.0, geometry_capacity_m) / minimum_positive))
      * max(2.0 * max(0.0, nozzle_exit_radius_m), full_throttle_length_m);
  envelope_center_y_m := -0.5 * envelope_length_m;
  // Use both sides of the authored signal: thrust is the delivered force, while
  // flow*velocity is the independently observable momentum estimate. The lower
  // value is the conservative exit-load estimate when mixture efficiency is
  // still settling during spool-up.
  momentum_flux_n := min(max(0.0, thrust_n),
    max(0.0, propellant_flow_kgs) * max(0.0, exhaust_velocity_mps))
    / max(1, engine_count);
  // One delivered momentum observation drives every presentation result.
  // min(thrust, mass-flow * exhaust velocity) is the conservative actual jet
  // load published as momentum_flux_n. Zero jet momentum gives zero activity by
  // ordinary normalization, including fuel exhaustion or a stale thrust value.
  thrust_fraction := min(1.0, max(0.0, momentum_flux_n) * max(1, engine_count)
    / max(minimum_positive, maximum_thrust_n));
  // A valve can have a short authored spool tail after a zero command. The
  // rendered plume follows delivered thrust, so zero thrust removes the light
  // and shader signal on this same equation path instead of leaving a ghost.
  t := min(min(1.0, max(0.0, throttle)), thrust_fraction);
  visual_t := max(0.0, t) ^ max(0.1, min(1.0, throttle_exponent));
  render_throttle := t;
  // The shader receives the derived metric, not a second throttle-shaped length
  // law. Its fixed cone is deliberately a capacity envelope; clamping here
  // keeps an unexpected design point from addressing outside that envelope.
  length := visual_t * full_throttle_length_m;
  visual_length_fraction := min(1.0, max(0.0, length)
    / max(minimum_positive, envelope_length_m));
  width := (width_idle + (1.0 - width_idle) * visual_t) * (max(0.0, w_max)
      + (1.0 - min(1.0, max(0.0, w_max) / minimum_positive)) * core_envelope_radius_m);
  exit_dynamic_pressure_pa := 0.5 * momentum_flux_n
    / max(minimum_positive, exit_area_m2);
  // The plume radiates from its flank, so the emitting surface is the cone's
  // lateral area — not its base, not its volume: A = pi*r*sqrt(r2+h2).
  area := pi * width * sqrt(width ^ 2 + length ^ 2);
  // The endpoint is exact: a dead engine emits no light even though the cone's
  // idle width gives its bounded geometry a non-zero lateral area.
  intensity := t * exitance * color_luminance * area;
  visual_intensity := visual_t * exitance * color_luminance * area;
  radius := r_idle + t * r_gain;
  visual_radius := r_idle + visual_t * r_gain;
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
    visual_intensity,
    radius,
    visual_radius,
    momentum_flux_n,
    exit_dynamic_pressure_pa};
end computePlumePhotometry;
