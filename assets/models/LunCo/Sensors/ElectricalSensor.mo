within LunCo.Sensors;

// Electrical Bus Voltage & Current Transducer Sensor Model.
// Reads EPS bus voltage and series-branch current, generating telemetry outputs
// and 12-bit ADC telemetry counts for Rhai power management and load-shedding scripts.
model ElectricalSensor
  extends LunCo.Icons.Sensor;
  parameter Real v_max_meas = 50.0 "Maximum voltage measurement range, V";
  parameter Real i_max_meas = 100.0 "Maximum current measurement range, A";

  LunCo.Electrical.Pin p "Upstream bus pin; positive current enters this sensor";
  LunCo.Electrical.Pin n "Downstream bus pin; ideal zero-voltage-drop current path";

  output Real v_bus_sensor "Measured EPS bus voltage reported to FSW, V";
  output Real i_bus_sensor "Measured EPS bus current reported to FSW, A";
  output Real adc_v_counts "Voltage 12-bit ADC count, 0..4095";
  output Real adc_i_counts "Current 12-bit ADC count, 0..4095";
equation
  p.v = n.v;
  p.i + n.i = 0.0;
  v_bus_sensor = p.v;
  i_bus_sensor = p.i;
  adc_v_counts = max(0.0, min(4095.0, (v_bus_sensor / v_max_meas) * 4095.0));
  adc_i_counts = max(0.0, min(4095.0, (abs(i_bus_sensor) / i_max_meas) * 4095.0));
end ElectricalSensor;
