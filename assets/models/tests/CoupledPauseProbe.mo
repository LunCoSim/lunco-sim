model CoupledPauseProbe "Observable state for coupled suspension acceptance"
  input Real input_value(unit = "m") = 1.0;
  output Real accumulated(unit = "m.s", start = 0.0);
equation
  der(accumulated) = input_value;
end CoupledPauseProbe;
