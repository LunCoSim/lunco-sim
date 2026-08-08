within LunCo.Sensors;

// A bounded-bandwidth differentiator for sampled or co-simulated measurements.
//
// A pure `der(input)` is not a valid reusable boundary for a signal arriving
// from another engine: the input has no state for the DAE structural pass to
// match. This first-order differentiator has one physical state and an
// exposed sensor bandwidth. The source can become valid after Modelica
// initialization, so the instrument has an explicit acquisition state: it
// tracks the first live sample without reporting the one-time zero-to-release
// handoff as physical acceleration. It is the same instrument model used by
// the IMU and altimeter, so both conversions remain structurally valid while
// retaining a meaningful measurement delay.
model FilteredDerivative
  extends LunCo.Icons.Sensor;

  parameter Real time_constant_s = 0.02
    "Measurement differentiator time constant (s)";
  parameter Real acquisition_time_constant_s = 0.15
    "Time for the first valid producer sample to enter the measurement (s)";
  input Real u = 0.0 "Measured signal";
  input Real sample_valid = 0.0
    "1 after the producer has published its first live sample";
  output Real y "Filtered derivative of the measured signal";

  Real state;
  Real validity "Continuous producer-validity gain";
  Real acquisition(start = 0.0)
    "Continuous confidence that the producer release sample is acquired";

equation
  // A first-order differentiator is a physical, continuous sensor state. Use
  // a numeric validity gain instead of a level comparison: fixed-step
  // co-simulation can update a Real input without scheduling a Modelica event,
  // while multiplication keeps the validity contract active in the same
  // continuous equation. Invalid data freezes the state and reports no
  // derivative; valid data follows the finite-bandwidth response.
  validity = max(0.0, min(1.0, sample_valid));
  der(acquisition) = (validity - acquisition)
    / max(1.0e-6, acquisition_time_constant_s);
  der(state) = validity * (u - state) / max(1.0e-6, time_constant_s);
  // Do not report the release handoff as acceleration. The low-pass state
  // acquires the first live value while confidence is still near zero; after
  // acquisition, the same derivative is the measured signal.
  y = validity * acquisition * der(state);

initial equation
  // The first solver sample is the instrument's reference velocity. This
  // avoids manufacturing a startup impulse from the producer's pre-release
  // placeholder while leaving subsequent changes to the continuous filter.
  state = u;
end FilteredDerivative;
