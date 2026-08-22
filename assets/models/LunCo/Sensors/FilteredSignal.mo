within LunCo.Sensors;

// A first-order measurement low-pass for a signal that is already sampled by
// the producer. It is intentionally separate from FilteredDerivative: an
// accelerometer receives solved acceleration, while a gyro's angular
// acceleration may still be derived from angular-rate samples.
model FilteredSignal
  extends LunCo.Icons.Sensor;

  parameter Real time_constant_s = 0.02
    "Measurement bandwidth time constant (s)";
  parameter Real acquisition_time_constant_s = 0.02
    "Time for a newly valid producer sample to enter the instrument (s)";
  input Real u = 0.0 "Measured signal";
  input Real sample_valid = 0.0
    "1 after the producer has published a complete sample";
  output Real y "Filtered measurement";

  Real state;
  Real validity;
  output Real acquisition(start = 0.0)
    "Continuous confidence that the first live sample is acquired";

equation
  validity = max(0.0, min(1.0, sample_valid));
  der(acquisition) = (validity - acquisition)
    / max(1.0e-6, acquisition_time_constant_s);
  // During acquisition, the sensor's internal state catches up faster than
  // its normal bandwidth. Once acquired, it becomes the authored first-order
  // measurement response. This prevents a co-simulation startup handoff from
  // appearing as a physical acceleration impulse.
  der(state) = validity * (u - state)
    / max(1.0e-6, time_constant_s * max(0.02, acquisition));
  y = u + acquisition * (state - u);

initial equation
  state = u;
  acquisition = 0.0;
end FilteredSignal;
