within LunCo.Pointing;
model EarthTracker "Two-axis high-gain antenna: hold the dish on Earth."
  // An ASSEMBLY, not a new law: two `ServoAxis` instances (the same component
  // the solar tracker uses) plus one `DishPattern`. The only equations written
  // here are the wiring and the geometry that connects them — which is the
  // point of a component library.
  //
  // ── Where the inputs come from ──────────────────────────────────────────
  // The direction to Earth in the VESSEL's frame, published by the engine's
  // celestial bridge exactly as `sun_azimuth` is published for the solar
  // tracker: the scene declares Earth as a celestial body
  // (`LunCoCelestialBodyAPI`, `lunco:body = 399`), the ephemeris places it, and
  // the bridge writes the local-frame direction each tick. Self-wired by name.
  //
  // Vessel attitude is therefore already accounted for: as the ship yaws — and
  // this episode spins it under the exposition beats — the local-frame
  // direction to Earth changes and the dish counter-rotates to hold the link.
  input Real earth_azimuth "direction to Earth, vessel frame (rad)";
  input Real earth_elevation "elevation of Earth above the vessel's horizon (rad)";

  parameter Real tau = 1.5 "gimbal time constant (s)";
  parameter Real diameter = 3.0 "reflector diameter (m) — must match the USD dish";
  parameter Real frequency = 2.2e9 "link frequency (Hz)";

  LunCo.Pointing.ServoAxis azimuth(tau = tau);
  LunCo.Pointing.ServoAxis elevation(tau = tau);
  LunCo.Pointing.DishPattern beam(diameter = diameter, frequency = frequency);

  // Gimbal angles for the actuator (`scenarios/dish_tracker.rhai`), which
  // applies them to the USD dish geometry.
  output Real az "dish azimuth setpoint (rad)";
  output Real el "dish elevation setpoint (rad)";
  // Link telemetry for the HUD's COMMS panel.
  output Real point_error "angle between boresight and Earth (rad)";
  output Real gain_frac "fraction of peak gain on the link, 0..1";
  output Real locked "1 while Earth is inside the half-power beam";
equation
  azimuth.cmd = earth_azimuth;
  elevation.cmd = earth_elevation;
  az = azimuth.angle;
  el = elevation.angle;

  // Great-circle separation between boresight and target. The cos(el) factor is
  // the convergence of azimuth lines toward the zenith — an azimuth error means
  // less on the sky the higher you point. Stays differentiable at zero, which an
  // acos() formulation does not.
  point_error = sqrt((earth_azimuth - az)^2
                     + (cos(el) * (earth_elevation - el))^2);
  beam.point_error = point_error;
  gain_frac = beam.gain_frac;
  locked = beam.locked;
end EarthTracker;
