model Battery
  parameter Real capacity = 100.0 "Total capacity in Ah";
  parameter Real voltage = 12.0 "Nominal voltage in V";
  Real soc(start=1.0) "State of Charge (0.0 to 1.0)";
  input Real current "Input current in Amperes (positive for discharge)";
  output Real soc_out;
equation
  der(soc) = -current / (capacity * 3600.0);
  soc_out = soc;
end Battery;
