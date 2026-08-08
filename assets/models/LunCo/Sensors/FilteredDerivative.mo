within LunCo.Sensors;

// A bounded-bandwidth differentiator for sampled or co-simulated measurements.
//
// A pure `der(input)` is not a valid reusable boundary for a signal arriving
// from another engine: the input has no state for the DAE structural pass to
// match. This first-order differentiator has one physical state and an
// exposed sensor bandwidth. It initializes that state from the first live
// measurement, which avoids a false startup impulse when an asynchronous
// co-simulation input becomes available after Modelica initialization. It is
// the same instrument model used by the IMU and altimeter, so both
// conversions remain structurally valid while retaining a meaningful
// measurement delay.
model FilteredDerivative
  extends LunCo.Icons.Sensor;

  parameter Real time_constant_s = 0.02
    "Measurement differentiator time constant (s)";
  input Real u = 0.0 "Measured signal";
  input Real sample_valid = 0.0
    "1 after the producer has published its first live sample";
  output Real y "Filtered derivative of the measured signal";

  Real state;
  Real validity;

equation
  // `sample_valid` is a confidence in [0, 1], so it gates both the state
  // update and the reported derivative continuously. This keeps the
  // cross-engine boundary branch-free while preserving the exact 0/1
  // behaviour of a producer that publishes a discrete validity flag. The
  // rising edge below still reinitializes the state to the first live value,
  // preventing an asynchronous handoff from looking like a physical impulse.
  validity = max(0.0, min(1.0, sample_valid));
  der(state) = validity * (u - state) / time_constant_s;
  y = validity * (u - state) / time_constant_s;

when sample_valid > 0.5 then
  reinit(state, u);
end when;

initial equation
  state = 0.0;
end FilteredDerivative;
