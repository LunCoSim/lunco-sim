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
  Real validity "Continuous producer-validity gain";

equation
  // A first-order differentiator is a physical, continuous sensor state. Use
  // a numeric validity gain instead of a level comparison: fixed-step
  // co-simulation can update a Real input without scheduling a Modelica event,
  // while multiplication keeps the validity contract active in the same
  // continuous equation. Invalid data freezes the state and reports no
  // derivative; valid data follows the finite-bandwidth response.
  validity = max(0.0, min(1.0, sample_valid));
  der(state) = validity * (u - state) / max(1.0e-6, time_constant_s);
  y = validity * der(state);

initial equation
  // The first solver sample is the instrument's reference velocity. This
  // avoids manufacturing a startup impulse from the producer's pre-release
  // placeholder while leaving subsequent changes to the continuous filter.
  state = u;
end FilteredDerivative;
