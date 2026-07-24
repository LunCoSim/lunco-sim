within LunCo.Logic;
block AboveThreshold
  "Branch-free 0..1 indication that a signal is above a threshold"
  parameter Real threshold = 0.0 "Activation threshold";
  parameter Real transition_width = 1.0 "Width of the clamped transition";
  input Real value = 0.0;
  output Real active "0 below, 1 above, linear across transition_width";
equation
  active = max(0.0, min(1.0,
    0.5 + (value - threshold) / max(transition_width, 1e-12)));
end AboveThreshold;
