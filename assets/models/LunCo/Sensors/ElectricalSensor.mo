within LunCo.Sensors;
import LunCo.Electrical.Pin;

// Electrical Bus Voltage & Current Transducer Sensor Model.
// Reads ground-truth EPS bus voltage and current from Pin, generating calibrated telemetry outputs
// and 12-bit ADC telemetry counts for Rhai power management and load-shedding scripts.
model ElectricalSensor
  parameter Real v_scale_factor = 0.10 "Voltage divider attenuation scale factor (0..5V scale)";
  parameter Real i_gain_v_a = 0.05 "Hall-effect current sensor sensitivity, V/A";
  parameter Real v_max_meas = 50.0 "Maximum voltage measurement range, V";
  parameter Real i_max_meas = 100.0 "Maximum current measurement range, A";

  Pin p "Electrical sensing pin";

  output Real v_bus_sensor "Measured EPS bus voltage reported to FSW, V";
  output Real i_bus_sensor "Measured EPS bus current reported to FSW, A";
  output Real adc_v_counts "Voltage 12-bit ADC count, 0..4095";
  output Real adc_i_counts "Current 12-bit ADC count, 0..4095";
equation
  v_bus_sensor = p.v;
  i_bus_sensor = p.i;
  adc_v_counts = max(0.0, min(4095.0, (v_bus_sensor / v_max_meas) * 4095.0));
  adc_i_counts = max(0.0, min(4095.0, (abs(i_bus_sensor) / i_max_meas) * 4095.0));
end ElectricalSensor;
