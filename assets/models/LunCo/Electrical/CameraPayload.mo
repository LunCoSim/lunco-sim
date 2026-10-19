within LunCo.Electrical;
model CameraPayload
  "Powered camera capture, electrical load and telemetry production"
  extends LunCo.Icons.Camera;
  extends RegulatedLoad;
  parameter Real p_active_w = 4.5 "Active capture input power, W";
  parameter Real p_idle_w = 0.2 "Powered standby draw, W";
  parameter Real data_rate_mbps = 15.0 "Full capture data production, Mbps";
  input Real active "Requested capture duty fraction, 0..1";
  output Real capture_active "Effective powered capture duty fraction";
  output Real data_rate_out_mbps "Actual generated telemetry, Mbps";
  output Real data_rate_out_bps(unit="bit/s");
  output Real heat_w(unit="W") "Electrical dissipation; outgoing data has no modeled energy flow";
equation
  assert(p_active_w >= p_idle_w and p_idle_w >= 0.0 and data_rate_mbps >= 0.0,
    "CameraPayload requires p_active_w >= p_idle_w >= 0 and a nonnegative data rate");
  capture_active = powered*max(0.0,min(1.0,active));
  requested_power_w = p_idle_w+(p_active_w-p_idle_w)*max(0.0,min(1.0,active));
  data_rate_out_mbps = data_rate_mbps*capture_active;
  data_rate_out_bps = data_rate_out_mbps*1.0e6;
  heat_w = power_draw_w;
end CameraPayload;
