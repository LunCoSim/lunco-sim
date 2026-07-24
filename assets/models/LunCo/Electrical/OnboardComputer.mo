within LunCo.Electrical;
import LunCo.Electrical.Pin;

// On-Board Computer (OBC / Flight Computer) baseline power draw & processing load.
model OnboardComputer
  parameter Real p_baseline_w = 12.0 "Flight computer baseline power draw, W";
  parameter Real p_gnc_w = 8.0 "Active GNC / hazard avoidance processing power draw, W";

  input Real gnc_active "GNC / Autopilot processing active state, 0..1";
  output Real power_draw_w "Total OBC electrical power draw, W";

  Pin p "Electrical bus pin";
equation
  power_draw_w = p_baseline_w + p_gnc_w * max(0.0, min(1.0, gnc_active));
  p.i = power_draw_w / max(1.0, p.v);
end OnboardComputer;
