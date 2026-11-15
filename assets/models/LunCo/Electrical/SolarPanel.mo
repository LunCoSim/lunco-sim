within LunCo.Electrical;
// A temperature-sensitive photovoltaic source on the bus. Reference efficiency
// and nameplate power are adjusted by cell temperature, incidence and visibility.
// `p.i` is negative because current leaves the panel into the electrical node.
model SolarPanel
  extends LunCo.Icons.SolarPanel;
  parameter Real area = 6.0 "Collecting area, m2";
  parameter Real efficiency = 0.30 "Irradiance-to-electrical conversion, 0..1";
  parameter Real reference_temperature_k(unit="K") = 298.15
    "Cell temperature at the reference efficiency and power rating";
  parameter Real temperature_curve_min_k(unit="K") = 0.0
    "Lower coefficient-evaluation bound; zero leaves the lower bound omitted";
  parameter Real temperature_curve_max_k(unit="K") = 0.0
    "Upper coefficient-validity bound; zero leaves the upper bound omitted";
  parameter Real power_temperature_coefficient_per_k(unit="1/K") = -0.0037
    "Relative power change per kelvin; generic PVWatts standard-silicon study value, not flight-cell calibration";
  parameter Real solar_absorptivity = 1.0
    "Absorbed fraction of incident radiation; default ideal opaque absorber";
  parameter Real peak_power_rating_w = area * efficiency * 1000.0
    "Peak output at standard test irradiance, W";
  // Module voltage at the maximum-power point. A PV module's photocurrent is set
  // by the light; this is the operating voltage that current is rated at, and it
  // is a property of the module, not of the bus it is bolted to.
  parameter Real v_mp = 48.0 "Module voltage at maximum power, V";

  input Real irradiance(unit="W/m2") "Unoccluded direct irradiance, W/m2";
  input Real cell_temperature_k(unit="K") = reference_temperature_k
    "Prescribed cell temperature, or feedback from a separate thermal network";
  input Real sun_visibility "Direct Sun visibility, 0..1; required physical input";
  input Real target_mount_x "Target direction in the panel frame, +X right";
  input Real target_mount_y "Target direction in the panel frame, +Y up";
  input Real target_mount_z "Target direction in the panel frame, -Z forward";
  input Real panel_normal_x "Panel illuminated-face normal in the panel local frame";
  input Real panel_normal_y "Panel illuminated-face normal in the panel local frame";
  input Real panel_normal_z "Panel illuminated-face normal in the panel local frame";

  Pin p annotation(Placement(transformation(origin={100,0}, extent={{-10,-10},{10,10}})));
  output Real power_out(unit="W") "Electrical power delivered to the bus after bus loading";
  output Real available_power_w(unit="W") "Electrical power available from incident sunlight before bus loading";
  output Real incident_irradiance_w_m2(unit="W/m2") "Direct irradiance on the collecting face after occlusion and incidence";
  output Real incident_power_w(unit="W") "Incident radiant power; thermal models apply their absorption and losses";
  output Real cos_incidence "Live clamped cosine of solar incidence, 0..1";
  output Real terminal_voltage_v(unit="V") "Electrical bus voltage at the solar-panel terminals";
  output Real generated_current_a(unit="A") "Current delivered from the panel to the electrical bus";
  output Real temperature_k(unit="K") "Actual cell temperature from the prescribed or thermal input";
  output Real curve_temperature_k(unit="K") "Temperature used to evaluate the electrical coefficient";
  output Real curve_limited "1 when the lower coefficient bound holds the evaluated temperature";
  output Real curve_in_range "1 when actual cell temperature is inside all authored coefficient bounds";
  output Real temperature_factor "Bounded relative power-temperature multiplier";
  output Real conversion_efficiency "Temperature-adjusted conversion before bus loading";
  output Real temperature_adjusted_peak_power_w(unit="W") "Reference power rating adjusted for cell temperature";
  output Real absorbed_power_w(unit="W") "Incident radiation absorbed by the panel";
  output Real reflected_power_w(unit="W") "Incident radiation not absorbed by the opaque panel";
  output Real heat_w(unit="W") "Absorbed radiation minus actual terminal electrical delivery; thermal-network input";

  Real panel_normal_norm "Magnitude of the authored panel normal";
  Real alignment "Signed alignment of the unit target direction and panel normal";
  Real unconstrained_available_power_w "Incident electrical power before a nameplate limit";
equation
  assert(area > 0.0 and v_mp > 0.0 and reference_temperature_k > 0.0
    and efficiency >= 0.0 and efficiency <= solar_absorptivity
    and solar_absorptivity <= 1.0 and peak_power_rating_w >= 0.0,
    "SolarPanel requires positive area, voltage and reference temperature, and 0 <= efficiency <= absorptivity <= 1");
  assert(temperature_curve_min_k >= 0.0 and temperature_curve_max_k >= 0.0
    and (temperature_curve_min_k <= 0.0 or temperature_curve_min_k <= reference_temperature_k)
    and (temperature_curve_max_k <= 0.0 or temperature_curve_max_k >= reference_temperature_k)
    and (temperature_curve_min_k <= 0.0 or temperature_curve_max_k <= 0.0
      or temperature_curve_min_k < temperature_curve_max_k),
    "SolarPanel coefficient bounds require nonnegative bounds, a reference inside the authored interval and lower < upper");
  assert(temperature_curve_max_k <= 0.0 or cell_temperature_k <= temperature_curve_max_k,
    "SolarPanel cell temperature exceeds the authored coefficient upper bound");
  assert(cell_temperature_k > 0.0 and irradiance >= 0.0
    and sun_visibility >= 0.0 and sun_visibility <= 1.0,
    "SolarPanel requires positive absolute cell temperature, nonnegative irradiance and visibility in [0,1]");
  panel_normal_norm = sqrt(max(1.0e-12,
    panel_normal_x^2 + panel_normal_y^2 + panel_normal_z^2));
  alignment = (
    target_mount_x * panel_normal_x
    + target_mount_y * panel_normal_y
    + target_mount_z * panel_normal_z
  ) / panel_normal_norm;
  cos_incidence = min(max(alignment, 0.0), 1.0);

  // The reduced I-V contract uses a current plateau below v_mp and a power
  // limit above it. Optical absorption and actual bus delivery close the heat
  // balance independently of the maximum available electrical power.
  incident_irradiance_w_m2 = irradiance * cos_incidence * sun_visibility;
  incident_power_w = area * incident_irradiance_w_m2;
  temperature_k = cell_temperature_k;
  // Lower-bound holding is an explicit reduced electrical assumption. The
  // actual thermal input is retained, and temperatures above an authored
  // upper validity bound fail rather than imply a calibrated hot tail.
  curve_temperature_k = max(cell_temperature_k, temperature_curve_min_k);
  curve_limited = if noEvent(cell_temperature_k < temperature_curve_min_k) then 1.0 else 0.0;
  curve_in_range = if noEvent((temperature_curve_min_k <= 0.0 or cell_temperature_k >= temperature_curve_min_k)
    and (temperature_curve_max_k <= 0.0 or cell_temperature_k <= temperature_curve_max_k)) then 1.0 else 0.0;
  temperature_factor = max(0.0, 1.0 + power_temperature_coefficient_per_k
    * (curve_temperature_k - reference_temperature_k));
  conversion_efficiency = min(solar_absorptivity, efficiency * temperature_factor);
  temperature_adjusted_peak_power_w = peak_power_rating_w * temperature_factor;
  unconstrained_available_power_w = conversion_efficiency * incident_power_w;
  available_power_w = min(
    unconstrained_available_power_w, temperature_adjusted_peak_power_w);
  // Flat current below the reference operating voltage, bounded power above
  // it. A high bus voltage cannot create more electrical energy than sunlight.
  p.i = -available_power_w / max(v_mp, p.v);

  // Delivered power FOLLOWS from the current and the bus voltage it actually
  // meets, so droop still shows up here — it is an output, not a driver.
  power_out = -p.i * p.v;
  terminal_voltage_v = p.v;
  generated_current_a = -p.i;
  absorbed_power_w = solar_absorptivity * incident_power_w;
  reflected_power_w = incident_power_w - absorbed_power_w;
  heat_w = absorbed_power_w - power_out;

end SolarPanel;
