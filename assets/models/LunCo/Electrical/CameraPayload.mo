within LunCo.Electrical;
import LunCo.Electrical.Pin;

// Navigation / Hazard / Science Camera Payload dynamics.
// Converts active capture state into electrical power draw on EPS bus Pin and data generation rate.
model CameraPayload
  parameter Real p_active_w = 4.5 "Active camera streaming power draw, W";
  parameter Real p_idle_w = 0.2 "Standby power draw, W";
  parameter Real data_rate_mbps = 15.0 "Science/Nav image data output rate, Mbps";

  input Real active "Camera active capture state (1.0 = streaming/capturing, 0.0 = standby)";
  output Real power_draw_w "Electrical power draw, W";
  output Real data_rate_out_mbps "Telemetry data output rate, Mbps";

  Pin p "Electrical bus pin";
equation
  power_draw_w = p_idle_w + (p_active_w - p_idle_w) * max(0.0, min(1.0, active));
  data_rate_out_mbps = data_rate_mbps * max(0.0, min(1.0, active));
  p.i = power_draw_w / max(1.0, p.v);
end CameraPayload;
