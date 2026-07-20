within LunCo.Pointing;
model SunTracker "Azimuth sun-tracker: yaw a panel to face the sun."
  // Now an ASSEMBLY over the shared `ServoAxis` rather than its own copy of
  // `der(yaw) = (cmd - yaw)/tau`. The dish tracker needs the identical servo on
  // two axes, so the servo became a component and both trackers instantiate it:
  // one law, one place to fix it, two vehicles' worth of behaviour.
  //
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
  output Real yaw "panel yaw setpoint (rad)";

  parameter Real tau = 2.0 "tracking time constant (s)";

  LunCo.Pointing.ServoAxis drive(tau = tau);
equation
  drive.cmd = sun_azimuth;
  yaw = drive.angle;
end SunTracker;
