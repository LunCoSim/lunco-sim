within LunCo.Comms;
model Transmitter
  "Regulated RF transmitter with explicit available and duty-averaged RF power"
  extends LunCo.Icons.Comms;
  extends LunCo.Electrical.RegulatedLoad;
  parameter Real p_rf_w = 5.0 "On-state RF output rating, W";
  parameter Real eta_tx = 0.40 "RF amplifier efficiency, (0,1]";
  parameter Real p_idle_w = 1.5 "Powered standby/controller draw, W";
  input Real tx_active "Requested transmit duty fraction, 0..1";
  output Real available_rf_power_w(unit="W")
    "Available on-state RF power for link acquisition/budget, independent of duty";
  output Real rf_power_out_w(unit="W") "Actual duty-averaged radiated RF power";
  output Real heat_w(unit="W") "Amplifier and standby dissipation";
equation
  assert(p_rf_w >= 0.0 and p_idle_w >= 0.0 and eta_tx > 0.0 and eta_tx <= 1.0,
    "Transmitter requires nonnegative power ratings and 0 < eta_tx <= 1");
  available_rf_power_w = powered*p_rf_w;
  rf_power_out_w = available_rf_power_w*max(0.0,min(1.0,tx_active));
  // An invalid efficiency must reach the assertion before any division by zero.
  requested_power_w = if eta_tx > 0.0 then
    p_idle_w + p_rf_w*max(0.0,min(1.0,tx_active))/eta_tx else 0.0;
  heat_w = power_draw_w-rf_power_out_w;
end Transmitter;
