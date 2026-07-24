within LunCo.Storage;
import LunCo.Electrical.Pin;

// Onboard Solid-State Mass Memory (SSMM / Flash Storage) dynamics.
// Tracks stored science data (GB), write/read power draw on EPS bus Pin, and storage fill.
model MassMemory
  parameter Real capacity_gb = 512.0 "Total mass memory capacity, GB";
  parameter Real p_standby_w = 0.5 "Standby power draw, W";
  parameter Real p_write_per_gbps = 3.5 "Write power draw per Gbps, W/(Gbps)";
  parameter Real p_read_per_gbps = 2.0 "Read power draw per Gbps, W/(Gbps)";

  input Real write_rate_gbps "Data write input rate from instruments, Gbps";
  input Real read_rate_gbps "Data read output rate to downlink, Gbps";

  Real stored_gb(start = 0.0) "Current stored science data volume, GB";
  output Real fill_pct "Storage fill percentage, 0..100 %";
  output Real power_draw_w "Total electrical power draw, W";

  Pin p "Electrical bus pin";
equation
  der(stored_gb) = max(-stored_gb, (write_rate_gbps - read_rate_gbps) / 8.0);
  fill_pct = (stored_gb / capacity_gb) * 100.0;
  power_draw_w = p_standby_w + p_write_per_gbps * max(0.0, write_rate_gbps) + p_read_per_gbps * max(0.0, read_rate_gbps);
  p.i = power_draw_w / max(1.0, p.v);
end MassMemory;
