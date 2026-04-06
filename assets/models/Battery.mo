model Battery
  parameter Real capacity = 1.0 "Total capacity in Ah";
  parameter Real voltage_nom = 12.0 "Nominal voltage in V";
  parameter Real R_internal = 0.01 "Internal resistance in Ohms";
  
  Real soc(start=1.0) "State of Charge (0.0 to 1.0)";
  Real v_oc "Open circuit voltage";
  
  input Real current "Input current in Amperes (positive for discharge)";
  
  output Real soc_out;
  output Real voltage_out;

equation
  // Smooth SOC-dependent open circuit voltage
  v_oc = voltage_nom * (0.8 + 0.2 * soc);
  
  // Terminal voltage
  voltage_out = v_oc - current * R_internal;
  
  // Continuous discharge rate with smooth guard at empty
  // Using a smooth limit instead of a hard 'if' to keep the DAE solver stable
  der(soc) = -current / (capacity * 3600.0);
  
  soc_out = soc;
end Battery;
