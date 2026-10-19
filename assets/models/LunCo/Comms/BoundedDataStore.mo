within LunCo.Comms;
model BoundedDataStore
  "Finite fluid queue with explicit accepted, delivered and rejected traffic"
  parameter Real capacity_bits(unit="bit") "Physical storage capacity";
  parameter Real initial_bits(unit="bit") = 0.0 "Initial retained data";
  input Real offered_rate_bps(unit="bit/s") "Incoming traffic offered for storage";
  input Real requested_rate_bps(unit="bit/s") "Requested outgoing traffic";
  output Real stored_bits(unit="bit", start=initial_bits, fixed=true);
  output Real accepted_rate_bps(unit="bit/s");
  output Real delivered_rate_bps(unit="bit/s");
  output Real rejected_rate_bps(unit="bit/s");
  output Real unserved_rate_bps(unit="bit/s");
equation
  assert(capacity_bits > 0.0 and initial_bits >= 0.0 and initial_bits <= capacity_bits,
    "BoundedDataStore requires positive capacity and an initial fill within capacity");
  assert(offered_rate_bps >= 0.0 and requested_rate_bps >= 0.0,
    "BoundedDataStore traffic requests must be nonnegative");
  delivered_rate_bps = if stored_bits <= 0.0 then min(requested_rate_bps, offered_rate_bps)
    else requested_rate_bps;
  accepted_rate_bps = if stored_bits >= capacity_bits then min(offered_rate_bps, delivered_rate_bps)
    else offered_rate_bps;
  rejected_rate_bps = offered_rate_bps - accepted_rate_bps;
  unserved_rate_bps = requested_rate_bps - delivered_rate_bps;
  der(stored_bits) = accepted_rate_bps - delivered_rate_bps;
end BoundedDataStore;
