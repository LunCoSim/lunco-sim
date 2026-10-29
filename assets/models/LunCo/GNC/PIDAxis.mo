within LunCo.GNC;

// One scalar PID feedback axis.  It is deliberately a Modelica component rather
// than three copied equations in the lander guidance model: the same block can be
// reused for X, Y, Z, attitude, or rover control, and its Logic icon makes the
// feedback structure visible in the Modelica diagram.
model PIDAxis
  extends LunCo.Icons.Logic;

  input Real enabled = 1.0 "One enables feedback; zero clears stored integral";
  parameter Real reset_time_s(min = 1.0e-9) = 0.25 "Integral reset time while disabled (s)";
  input Real setpoint = 0.0 "Desired value";
  input Real measurement = 0.0 "Measured value";
  input Real setpoint_rate = 0.0 "Desired rate";
  input Real measurement_rate = 0.0 "Measured rate";

  input Real kp = 1.0 "Proportional gain";
  input Real ki = 0.0 "Integral gain";
  input Real kd = 0.0 "Derivative gain";
  input Real integral_limit = 10.0 "Integral state limit";
  input Real output_limit = 1.0 "Command limit";
  input Real anti_windup_gain = 1.0 "Back-calculation gain";

  output Real error "Setpoint minus measurement";
  output Real command "Saturated PID command";
  output Real integral "Bounded integral state";

  Real integral_state(start = 0.0, fixed = true) "Internal integral state";
  Real integral_rate "Error and output-saturation back-calculation";
  Real raw_command "Unsaturated PID command";
  Real enabled_gate;
  Real saturated_command "Bounded command used by the controller";
  Real rate_error "Desired rate minus measured rate";

equation
  enabled_gate = max(0.0, min(1.0, enabled));
  error = setpoint - measurement;
  rate_error = setpoint_rate - measurement_rate;

  // Back-calculation anti-windup.  The integrator is part of the simulation
  // state, so changing gains in the Inspector changes the live controller
  // without replacing the model or asking Rhai to perform control work.
  // The command and anti-windup equations are the complete PID contract; the
  // unsaturated expression is intentionally not a separate public signal.
  raw_command = kp * error + ki * integral + kd * rate_error;
  saturated_command = max(-output_limit, min(output_limit, raw_command));
  command = enabled_gate * saturated_command;
  integral_rate = error
    + anti_windup_gain * (saturated_command - raw_command);
  // Integrate inward at either bound, but do not accumulate hidden windup
  // while the bounded integral contribution is already at its limit.
  der(integral_state) = noEvent(if enabled_gate < 0.5
    then -integral_state / reset_time_s
    else if integral_state >= max(0.0, integral_limit)
    then min(0.0, integral_rate)
    else if integral_state <= -max(0.0, integral_limit)
    then max(0.0, integral_rate) else integral_rate);
  integral = max(-max(0.0, integral_limit),
    min(max(0.0, integral_limit), integral_state));
end PIDAxis;
