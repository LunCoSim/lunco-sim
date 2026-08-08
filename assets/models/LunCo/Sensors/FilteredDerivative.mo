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

equation
  // Before the producer is live, hold the filter state and report no
  // derivative. The rising edge reinitializes the state to the first live
  // value, so an async co-sim handoff cannot look like a physical acceleration
  // impulse. Keeping the state differential in both branches is important:
  // switching a state between an algebraic equation and der(state) leaves the
  // live solver with a singular/non-finite derivative at release.
  der(state) = if sample_valid > 0.5
    then (u - state) / time_constant_s
    else 0.0;
  y = if sample_valid > 0.5
    then (u - state) / time_constant_s
    else 0.0;

when sample_valid > 0.5 then
  reinit(state, u);
end when;

initial equation
  state = 0.0;
end FilteredDerivative;
