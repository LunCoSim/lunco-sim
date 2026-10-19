within LunCo.Comms;
model DataBuffer
  "Drop-tail telemetry buffer; decimal MB and kbps"
  extends LunCo.Icons.DataBuffer;
  parameter Real capacity_mb = 1024.0 "Telemetry buffer capacity, MB";
  parameter Real buffer_init_mb = 0.0 "Initial buffer fill, MB";
  input Real rate_in_kbps "Offered payload traffic, kbps";
  input Real rate_out_kbps "Requested outgoing traffic, kbps";
  output Real buffer_mb "Retained data, MB";
  output Real buffer_fill_pct "Buffer fill, percent";
  output Real buffer_full "1 when capacity is reached";
  output Real stored_data_mb(unit="MB");
  output Real free_capacity_mb(unit="MB");
  output Real accepted_rate_kbps "Incoming traffic retained or immediately forwarded";
  output Real delivered_rate_kbps "Actual outgoing traffic";
  output Real dropped_rate_kbps "Offered traffic rejected at capacity";
  output Real unserved_rate_kbps "Requested outgoing traffic unavailable at empty";
  LunCo.Comms.BoundedDataStore store(capacity_bits=capacity_mb*8.0e6,
    initial_bits=buffer_init_mb*8.0e6,
    offered_rate_bps=rate_in_kbps*1000.0,
    requested_rate_bps=rate_out_kbps*1000.0);
equation
  buffer_mb = store.stored_bits/8.0e6;
  stored_data_mb = buffer_mb;
  free_capacity_mb = capacity_mb-buffer_mb;
  // Invalid capacities are rejected by the store assertion. Keep presentation
  // equations defined until the solver reports that diagnostic.
  buffer_fill_pct = if capacity_mb > 0.0 then 100.0*buffer_mb/capacity_mb else 0.0;
  buffer_full = if store.stored_bits >= store.capacity_bits then 1.0 else 0.0;
  accepted_rate_kbps = store.accepted_rate_bps/1000.0;
  delivered_rate_kbps = store.delivered_rate_bps/1000.0;
  dropped_rate_kbps = store.rejected_rate_bps/1000.0;
  unserved_rate_kbps = store.unserved_rate_bps/1000.0;
end DataBuffer;
