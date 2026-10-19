within LunCo.Comms;
model CommsLink
  "Friis RF budget, modem availability and finite drop-tail telemetry queue"
  constant Real c = 299792458.0 "Speed of light, m/s";
  constant Real k_B = 1.380649e-23 "Boltzmann constant, J/K";
  constant Real pi = 3.141592653589793;
  input Real tx_power_w = 10.0
    "Available on-state carrier power, W; wire Transmitter.available_rf_power_w";
  parameter Real gain_tx_db = 3.0 "Transmit antenna gain, dBi";
  parameter Real gain_rx_db = 30.0 "Receive antenna gain, dBi";
  parameter Real freq_hz = 2.2e9 "Carrier frequency, Hz";
  parameter Real bandwidth_hz = 2.0e6 "Channel bandwidth, Hz";
  parameter Real noise_temp_k = 150.0 "System noise temperature, K";
  parameter Real line_loss_db = 2.0 "Implementation losses, dB";
  parameter Real rate_efficiency = 0.6 "Achievable fraction of Shannon capacity";
  parameter Real snr_required_db = 3.0 "Demodulation threshold, dB";
  input Real data_in_bps = 5.0e5 "Actual offered payload traffic, bits/s";
  parameter Real buffer_capacity_bits = 8.0e7
    "Fixed physical buffer capacity; changing the device requires reinitialization";
  parameter Real buffer_release_time_s = 1.0
    "Modem queue-release time constant, s";
  parameter Real T_filter = 0.25 "Geometry input filter time constant, s";
  parameter Real range_min_m = 1.0 "Minimum physical range admitted by the budget, m";
  parameter Real conn_band = 0.01 "Width of geometry availability transition";
  parameter Real margin_band_db = 0.1 "Width of demodulation transition, dB";
  input Real link_range_m "Selected peer range from geometry, m";
  input Real link_connected "Geometry visibility, 0..1";
  output Real rate_bps "Available downlink capacity, bits/s";
  output Real delivered_rate_bps "Actual transmitted traffic, bits/s";
  output Real accepted_rate_bps "Incoming traffic accepted by the queue, bits/s";
  output Real dropped_rate_bps "Traffic rejected at capacity, bits/s";
  output Real snr_valid "1 when carrier power is positive and dB values are defined";
  output Real snr_db "SNR in dB while snr_valid=1; zero placeholder otherwise";
  output Real margin_db "Demodulation margin while snr_valid=1; zero placeholder otherwise";
  output Real buffer_bits "Retained telemetry, bits";
  output Real buffer_frac "Fraction of physical buffer capacity occupied";
  output Real lost_bits(start=0.0, fixed=true) "Cumulative dropped telemetry, bits";
  output Real up "Geometry and demodulation availability, 0..1";
  output Real tx_active "Duty-averaged transmit demand, 0..1";
protected
  Real range_f(start=1.0e6, fixed=true);
  Real conn_f(start=0.0, fixed=true);
  Real snr_linear;
  Real modem_capacity_bps;
  Real conn_gate;
  Real margin_gate;
  LunCo.Comms.BoundedDataStore store(capacity_bits=buffer_capacity_bits,
    offered_rate_bps=data_in_bps,
    requested_rate_bps=if buffer_release_time_s > 0.0 then
      min(rate_bps,data_in_bps+max(0.0,store.stored_bits)/buffer_release_time_s) else 0.0);
equation
  assert(tx_power_w >= 0.0 and freq_hz > 0.0 and bandwidth_hz > 0.0 and noise_temp_k > 0.0,
    "CommsLink requires nonnegative carrier power and positive RF frequency, bandwidth and noise temperature");
  assert(rate_efficiency > 0.0 and rate_efficiency <= 1.0,
    "CommsLink requires 0 < rate_efficiency <= 1");
  assert(buffer_release_time_s > 0.0 and T_filter > 0.0 and range_min_m > 0.0
    and conn_band > 0.0 and margin_band_db > 0.0,
    "CommsLink time constants, range floor and transition widths must be positive");
  T_filter*der(range_f)+range_f = max(link_range_m,range_min_m);
  T_filter*der(conn_f)+conn_f = max(0.0,min(1.0,link_connected));
  snr_valid = if tx_power_w > 0.0 then 1.0 else 0.0;
  // The log-domain Friis budget avoids received-power underflow. The positive
  // argument and its validity test share the same carrier value, including at
  // power-switching events. Off-state dB placeholders are explicitly invalid.
  snr_db = if tx_power_w > 0.0 then
    10.0*log10(noEvent(if tx_power_w > 0.0 then tx_power_w else 1.0))
    +gain_tx_db+gain_rx_db-line_loss_db
    +20.0*(log10(c)-log10(freq_hz)-log10(4.0*pi)-log10(max(range_f,range_min_m)))
    -10.0*(log10(k_B)+log10(noise_temp_k)+log10(bandwidth_hz)) else 0.0;
  snr_linear = noEvent(if tx_power_w > 0.0 then tx_power_w else 0.0)
    *10.0^((gain_tx_db+gain_rx_db-line_loss_db)/10.0)
    *(c/(4.0*pi*freq_hz*max(range_f,range_min_m)))^2
    /(k_B*noise_temp_k*bandwidth_hz);
  margin_db = if tx_power_w > 0.0 then snr_db-snr_required_db else 0.0;
  modem_capacity_bps = rate_efficiency*bandwidth_hz*log(1.0+snr_linear)/log(2.0);
  conn_gate = max(0.0,min(1.0,(conn_f-0.5)/conn_band));
  margin_gate = if tx_power_w > 0.0 then max(0.0,min(1.0,margin_db/margin_band_db)) else 0.0;
  up = conn_gate*margin_gate;
  rate_bps = up*modem_capacity_bps;
  accepted_rate_bps = store.accepted_rate_bps;
  delivered_rate_bps = store.delivered_rate_bps;
  dropped_rate_bps = store.rejected_rate_bps;
  der(lost_bits) = dropped_rate_bps;
  buffer_bits = store.stored_bits;
  buffer_frac = if buffer_capacity_bits > 0.0 then buffer_bits/buffer_capacity_bits else 0.0;
  tx_active = noEvent(if modem_capacity_bps > 0.0 then delivered_rate_bps/modem_capacity_bps else 0.0);
end CommsLink;
