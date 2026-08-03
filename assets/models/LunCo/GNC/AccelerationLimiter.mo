within LunCo.GNC;

// Reusable acceleration limiter for guidance and actuator command paths.
// Keeping saturation as a named Modelica block makes its limits visible in the
// diagram and lets every guidance model use the same bounded signal contract.
model AccelerationLimiter
  extends LunCo.Icons.Logic;

  input Real command "Requested acceleration (m/s²)";
  input Real lower_limit = 0.0 "Minimum allowed acceleration (m/s²)";
  input Real upper_limit = 8.0 "Maximum allowed acceleration (m/s²)";
  output Real bounded_command "Bounded acceleration (m/s²)";
  Real bounded_value "Internal bounded acceleration";

equation
  bounded_value = max(lower_limit, min(upper_limit, command));
  bounded_command = bounded_value;
end AccelerationLimiter;
