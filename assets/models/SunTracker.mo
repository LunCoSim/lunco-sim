model SunTracker "Azimuth sun-tracker: yaw a panel to face the sun."
  // Live sun azimuth (rad). Fed from the engine's solar bridge
  // (`lunco_environment::inject_local_solar_into_cosim`, which publishes the
  // scene sun direction as a `SimComponent` output `sun_azimuth`) through the
  // self-wire `sun_azimuth -> sun_azimuth` — same name on both sides: the
  // bridge's output port and this input port are the one signal. (The per-tick
  // output sync may echo this input back into the outputs map, but only ever
  // with the value the bridge just fed in, so the loop is a stable fixed point
  // at the live sun azimuth.)
  input Real sun_azimuth "commanded sun azimuth (rad)";

  // Panel yaw setpoint (rad). Wired to the yaw joint's `angle` input port
  // (`yaw -> angle`); the joint's angular motor realizes it.
  output Real yaw(start = 0.0) "panel yaw setpoint (rad)";

  // Tracking time constant: how fast the head chases the sun. Small = snappy,
  // large = lazy. A real first-order servo, so the head eases onto the sun
  // instead of snapping — and `der(yaw)` exercises the ODE solver end-to-end.
  parameter Real tau = 2.0 "tracking time constant (s)";
equation
  der(yaw) = (sun_azimuth - yaw) / tau;
end SunTracker;
