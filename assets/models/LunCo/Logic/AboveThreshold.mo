within LunCo.Logic;
block AboveThreshold "Branch-free 0..1 indication that a signal is above a threshold"
  extends LunCo.Icons.Logic;
  // Runtime inputs keep the threshold usable as a live, reusable signal block.
  // A parent model can expose the values to USD/Inspector and connect them
  // explicitly; component modifiers do not silently freeze an outer input into
  // a child parameter during live co-simulation.
  input Real threshold = 0.0 "Activation threshold";
  input Real transition_width = 1.0 "Width of the clamped transition";
  input Real minimum_transition_width = 1.0e-12
    "Smallest permitted transition width";
  input Real value = 0.0;
  output Real active "0 below, 1 above, linear across transition_width";
equation
  active = max(0.0, min(1.0,
    0.5 + (value - threshold)
      / max(transition_width, minimum_transition_width)));
end AboveThreshold;
