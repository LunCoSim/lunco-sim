model Battery
  parameter Real capacity = 1.0 "Total capacity in Ah";
  parameter Real voltage_nom = 12.0 "Nominal voltage in V";
  parameter Real R_internal = 0.01 "Internal resistance in Ohms";
  parameter Real T_filter = 0.1 "Input filter time constant for stability";
  
  Real soc(start=1.0) "State of Charge (0.0 to 1.0)";
  Real v_oc "Open circuit voltage";
  Real current(start=0.0) "Filtered current";
  Real current_saturated "Safety limited current";
  
  input Real current_in "Raw input current in Amperes";
  
  output Real soc_out;
  output Real voltage_out;

equation
  // Input Saturation: Prevent astronomical inputs from breaking the solver
  current_saturated = if current_in > 1000 then 1000 else if current_in < -1000 then -1000 else current_in;

  // Input filtering: Converts jumps into smooth transitions for the solver
  T_filter * der(current) + current = current_saturated;

  // Smooth SOC-dependent open circuit voltage
  v_oc = voltage_nom * (0.8 + 0.2 * (if soc > 0.0 then soc else 0.0));
  
  // Terminal voltage
  voltage_out = v_oc - current * R_internal;
  
  // Charge balance
  der(soc) = -current / (capacity * 3600.0);
  
  soc_out = soc;
end Battery;
