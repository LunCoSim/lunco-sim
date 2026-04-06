model RC_Circuit
  parameter Real R = 1000.0 "Resistance in Ohms";
  parameter Real C = 1e-6 "Capacitance in Farads";
  
  input Real v_in "Source voltage in Volts";
  Real v_c(start=0.0) "Capacitor voltage in Volts";

equation
  C * der(v_c) = (v_in - v_c) / R;
end RC_Circuit;
