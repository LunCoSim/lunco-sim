within LunCo.Comms;
import LunCo.Electrical.Pin;

// RF Transmitter: converts transmit state into electrical power draw on EPS bus Pin.
model Transmitter
  parameter Real p_rf_w = 5.0 "RF output transmit power, W";
  parameter Real eta_tx = 0.40 "Transmitter RF efficiency, 0..1";
  parameter Real p_idle_w = 1.5 "Idle receiver standby power draw, W";

  input Real tx_active "Transmitter active state (1.0 = transmitting, 0.0 = receiving/standby)";
  output Real power_draw_w "Total electrical power draw, W";
  output Real rf_power_out_w "RF power radiated, W";

  Pin p "Electrical bus pin";
equation
  rf_power_out_w = p_rf_w * max(0.0, min(1.0, tx_active));
  power_draw_w = p_idle_w + (rf_power_out_w / max(0.01, eta_tx));
  p.i = power_draw_w / max(1.0, p.v);
end Transmitter;
