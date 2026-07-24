within LunCo.Thermal;
import LunCo.Thermal.HeatPort;
import LunCo.Electrical.Pin;

// Thermo-electrical survival heater.
// Draws electrical power from EPS bus Pin when component temperature drops below setpoint T_set.
model ThermostatHeater
  parameter Real t_set_k = 263.15 "Thermostat turn-on temperature setpoint (-10°C), K";
  parameter Real p_heater_w = 15.0 "Electric heater power capacity, W";
  parameter Real eta_heat = 0.98 "Electrical to thermal heating conversion efficiency, 0..1";

  output Real heater_active "Heater status (1.0 = active heating, 0.0 = off)";
  output Real heat_out_w "Thermal heat flow into component, W";

  HeatPort thermal_port "Thermal heat output port";
  Pin elec_port "Electrical power supply pin";
equation
  heater_active = max(0.0, min(1.0, 0.5 + t_set_k - thermal_port.T));
  heat_out_w = p_heater_w * eta_heat * heater_active;
  thermal_port.Q = -heat_out_w;
  elec_port.i = (p_heater_w * heater_active) / max(1.0, elec_port.v);
end ThermostatHeater;
