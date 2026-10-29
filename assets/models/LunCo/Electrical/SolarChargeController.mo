within LunCo.Electrical;
model SolarChargeController
  "Reduced solar converter: regulated PV input, lossy power transfer to the bus"
  extends LunCo.Icons.ElectricalControl;
  // Mission-level operating-point model, not switching electronics or a
  // manufacturer-specific MPPT/BMS algorithm. Author the PV operating voltage
  // and efficiency from measured equipment data when those data are available.
  parameter Real pv_voltage_v(unit="V") = 48.0;
  parameter Real efficiency = 0.95 "Assumed conversion efficiency, 0..1";
  parameter Real minimum_bus_voltage_v(unit="V") = 18.0;
  Pin pv "Input from the photovoltaic source";
  Pin bus "Output to the battery and loads";
  output Real input_power_w(unit="W");
  output Real output_power_w(unit="W");
  output Real heat_w(unit="W");
equation
  assert(pv_voltage_v > 0.0 and minimum_bus_voltage_v > 0.0
      and efficiency > 0.0 and efficiency <= 1.0,
      "SolarChargeController requires positive operating voltages and efficiency in (0,1]");
  pv.v = pv_voltage_v;
  input_power_w = pv.v * pv.i;
  bus.i = if bus.v > minimum_bus_voltage_v
    then -efficiency * max(input_power_w, 0.0) / bus.v else 0.0;
  output_power_w = -bus.v * bus.i;
  // Below the bus operating threshold this reduced model rejects PV power as
  // converter heat. It does not claim hardware start-up or full-charge control.
  heat_w = input_power_w - output_power_w;
end SolarChargeController;
