within LunCo.Electrical;
model HeadlightController
  "Powered photometric lamp facet; USD authors its nominal luminous output"
  extends LunCo.Icons.ElectricalControl;
  extends RegulatedLoad;
  parameter Real nominal_power_w = 24.0 "Full-on electrical draw, W";
  parameter Real luminous_intensity_lm = 120000.0 "Full-on luminous output, lm";
  input Real enable "Requested lamp duty fraction, 0..1";
  output Real enabled "Effective powered lamp duty fraction";
  output Real light_intensity(unit="lm") "Actual luminous output for the USD light";
equation
  assert(nominal_power_w > 0.0 and luminous_intensity_lm >= 0.0,
    "HeadlightController requires positive input power and nonnegative luminous output");
  enabled = powered*max(0.0,min(1.0,enable));
  light_intensity = luminous_intensity_lm*enabled;
  requested_power_w = nominal_power_w*max(0.0,min(1.0,enable));
end HeadlightController;
