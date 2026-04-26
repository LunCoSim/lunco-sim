model Oscillator
  parameter Real frequency = 1.0 "Hz";
  parameter Real amplitude = 1.0 "Peak amplitude";
  // Phase is the integrated state — rumoca's solver rejects models
  // with no state variable as `EmptySystem`, so we drive the
  // oscillator by integrating angular velocity rather than reading
  // `time` directly. This also lets simulation pause/reset cleanly.
  Real phase(start = 0.0) "Accumulated phase in radians";
  output Real signal "Sine wave at `frequency` Hz with peak `amplitude`";
equation
  der(phase) = 2.0 * 3.14159265358979 * frequency;
  signal = amplitude * sin(phase);
end Oscillator;
