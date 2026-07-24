within LunCo.Comms;
// Acausal telemetry data stream connector: data rate in kbps and data flow rate.
connector DataPort
  Real rate_kbps "Current data transfer rate, kbps";
  flow Real flow_kbps "Data flow entering connector, kbps (summed to zero at node)";
end DataPort;
