within LunCo.Sensors;
import LunCo.Thermal.HeatPort;

// RTD / Thermocouple Temperature Sensor Model.
// Measures component ground-truth temperature from HeatPort, modeling sensor thermal response lag,
// calibration offset, and 12-bit ADC telemetry count outputs for thermal control scripts.
model ThermalSensor
  parameter Real cal_offset_k = 0.35 "Sensor calibration offset bias, K";
  parameter Real tau_sec = 2.0 "Sensor thermal probe response time constant, s";
  parameter Real t_min_k = 100.0 "ADC minimum scale temperature, K";
  parameter Real t_max_k = 450.0 "ADC maximum scale temperature, K";

  HeatPort port "Thermal heat sensing port";

  Real t_sensed_k(start = 293.15) "Internal probe temperature with response lag, K";
  output Real temp_sensor_k "Sensed temperature reported to Rhai thermal controller, K";
  output Real temp_sensor_c "Sensed temperature in Celsius, °C";
  output Real adc_counts "12-bit ADC telemetry count, 0..4095";
equation
  der(t_sensed_k) = (port.T - t_sensed_k) / max(0.1, tau_sec);
  temp_sensor_k = t_sensed_k + cal_offset_k;
  temp_sensor_c = temp_sensor_k - 273.15;
  adc_counts = max(0.0, min(4095.0, ((temp_sensor_k - t_min_k) / (t_max_k - t_min_k)) * 4095.0));
end ThermalSensor;
