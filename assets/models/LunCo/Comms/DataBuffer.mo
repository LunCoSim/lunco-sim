within LunCo.Comms;
// Onboard telemetry data buffer dynamics: der(buffer_mb) = (rate_in_kbps - rate_out_kbps) / 8000
model DataBuffer
  parameter Real capacity_mb = 1024.0 "Telemetry buffer capacity, MB";
  parameter Real buffer_init_mb = 0.0 "Initial buffer fill at t=0, MB";

  input Real rate_in_kbps "Payload data generation rate, kbps";
  input Real rate_out_kbps "Downlink transmission data rate, kbps";

  Real buffer_mb(start = buffer_init_mb) "Current buffer storage fill, MB";
  output Real buffer_fill_pct "Buffer fill percentage, 0..100 %";
  output Real buffer_full "Buffer overflow flag (1.0 = full, 0.0 = space available)";
equation
  der(buffer_mb) = max(-buffer_mb, (rate_in_kbps - rate_out_kbps) / 8000.0);
  buffer_fill_pct = (buffer_mb / capacity_mb) * 100.0;
  buffer_full = max(0.0, min(1.0, (buffer_mb - capacity_mb + 1.0)));
end DataBuffer;
