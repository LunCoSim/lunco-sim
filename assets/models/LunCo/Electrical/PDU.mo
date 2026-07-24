within LunCo.Electrical;
import LunCo.Electrical.Pin;

// Electrical Power Subsystem (EPS) Power Distribution Unit (PDU).
// Regulates 28V main bus voltage, manages solar generation & battery charge/discharge balance,
// and enforces under-voltage load shedding when battery SOC drops below critical threshold.
model PDU
  parameter Real v_bus_target = 28.0 "Regulated EPS main bus voltage, V";
  parameter Real p_max_w = 1200.0 "Maximum continuous PDU power rating, W";
  parameter Real soc_cutoff = 0.10 "Low-charge cutoff threshold for non-essential loads, 0..1";

  input Real p_solar_in "Incoming solar array power generation, W";
  input Real bat_soc "Current battery state of charge, 0..1";

  output Real v_bus "Regulated bus voltage output, V";
  output Real load_shedding "Load shedding flag (1.0 = shed non-essential loads, 0.0 = normal operation)";
  output Real net_power_w "Net power flow (positive = charging battery, negative = discharging), W";

  Pin p "Electrical main bus pin";
equation
  v_bus = v_bus_target * max(0.0, min(1.0, 100.0 * (bat_soc - 0.02)));
  load_shedding = max(0.0, min(1.0, 0.5 + 100.0 * (soc_cutoff - bat_soc)));
  net_power_w = p_solar_in - (p.i * v_bus);
  p.v = v_bus;
end PDU;
