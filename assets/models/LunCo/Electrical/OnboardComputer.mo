within LunCo.Electrical;
model OnboardComputer
  "Regulated flight computer load and powered processing availability"
  extends LunCo.Icons.ElectricalControl;
  extends RegulatedLoad;
  parameter Real p_baseline_w = 12.0 "Powered baseline draw, W";
  parameter Real p_gnc_w = 8.0 "Additional full GNC processing draw, W";
  input Real gnc_active "Requested processing duty fraction, 0..1";
  output Real processing_active "Effective powered processing fraction";
  output Real heat_w(unit="W");
equation
  assert(p_baseline_w >= 0.0 and p_gnc_w >= 0.0,
    "OnboardComputer power coefficients must be nonnegative");
  processing_active = powered*max(0.0,min(1.0,gnc_active));
  requested_power_w = p_baseline_w+p_gnc_w*max(0.0,min(1.0,gnc_active));
  heat_w = power_draw_w;
end OnboardComputer;
