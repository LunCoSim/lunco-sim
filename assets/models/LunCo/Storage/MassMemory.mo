within LunCo.Storage;
model MassMemory
  "Nonvolatile mass memory with finite capacity and powered read/write service"
  extends LunCo.Icons.Storage;
  extends LunCo.Electrical.RegulatedLoad;
  parameter Real capacity_gb = 512.0 "Physical capacity, decimal GB";
  parameter Real stored_init_gb = 0.0 "Initially retained data, GB";
  parameter Real p_standby_w = 0.5 "Powered controller standby draw, W";
  parameter Real p_write_per_gbps = 3.5 "Accepted-write energy coefficient, W/Gbps";
  parameter Real p_read_per_gbps = 2.0 "Delivered-read energy coefficient, W/Gbps";
  input Real write_rate_gbps "Offered write requests, Gbps";
  input Real read_rate_gbps "Requested outgoing traffic, Gbps";
  output Real stored_gb "Retained data, GB";
  output Real fill_pct "Storage fill, percent";
  output Real stored_data_gb(unit="GB");
  output Real free_capacity_gb(unit="GB");
  output Real accepted_write_rate_gbps "Actual accepted write service";
  output Real delivered_read_rate_gbps "Actual outgoing service";
  output Real rejected_write_rate_gbps "Offered writes not accepted; caller may retry";
  output Real unserved_read_rate_gbps "Requested reads not delivered";
  output Real heat_w(unit="W") "Electrical input dissipated in the memory/controller";
  LunCo.Comms.BoundedDataStore store(capacity_bits=capacity_gb*8.0e9,
    initial_bits=stored_init_gb*8.0e9,
    offered_rate_bps=powered*write_rate_gbps*1.0e9,
    requested_rate_bps=powered*read_rate_gbps*1.0e9);
equation
  assert(write_rate_gbps >= 0.0 and read_rate_gbps >= 0.0,
    "MassMemory traffic requests must be nonnegative");
  assert(p_standby_w >= 0.0 and p_write_per_gbps >= 0.0 and p_read_per_gbps >= 0.0,
    "MassMemory power coefficients must be nonnegative");
  stored_gb = store.stored_bits/8.0e9;
  stored_data_gb = stored_gb;
  free_capacity_gb = capacity_gb-stored_gb;
  fill_pct = if capacity_gb > 0.0 then 100.0*stored_gb/capacity_gb else 0.0;
  accepted_write_rate_gbps = store.accepted_rate_bps/1.0e9;
  delivered_read_rate_gbps = store.delivered_rate_bps/1.0e9;
  rejected_write_rate_gbps = write_rate_gbps-accepted_write_rate_gbps;
  unserved_read_rate_gbps = read_rate_gbps-delivered_read_rate_gbps;
  requested_power_w = p_standby_w + p_write_per_gbps*accepted_write_rate_gbps
    + p_read_per_gbps*delivered_read_rate_gbps;
  heat_w = power_draw_w;
end MassMemory;
