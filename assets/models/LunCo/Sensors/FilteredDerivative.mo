within LunCo.Sensors;

// A bounded-bandwidth differentiator for sampled or co-simulated measurements.
//
// A pure `der(input)` is not a valid reusable boundary for a signal arriving
// from another engine: the input has no state for the DAE structural pass to
// match. This first-order differentiator has one physical state, an exposed
// sensor bandwidth, and initializes from the first measurement. It is the
// same instrument model used by the IMU and altimeter, so both conversions
// remain structurally valid while retaining a meaningful measurement delay.
model FilteredDerivative
  extends LunCo.Icons.Sensor;

  parameter Real time_constant_s = 0.02
    "Measurement differentiator time constant (s)";

  input Real u = 0.0 "Measured signal";
  output Real y "Filtered derivative of the measured signal";

  Real state;

equation
  der(state) = (u - state) / time_constant_s;
  y = (u - state) / time_constant_s;

initial equation
  state = u;
end FilteredDerivative;
