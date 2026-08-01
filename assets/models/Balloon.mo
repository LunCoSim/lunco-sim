// Lighter-than-air envelope: buoyancy and drag against the AMBIENT MEDIUM.
//
// THE MEDIUM IS AN INPUT, NOT A CONSTANT. This model used to derive air density
// from a hard-coded sea-level pressure and multiply it by `g = 9.81` — Earth's
// atmosphere and Earth's gravity, baked in. Instantiated into the lunar sandbox
// (`PhysicsScene` gravity 1.62 m/s²) that produced 71 N of buoyancy against 7.3 N
// of weight: a 9.8× net lift with no altitude at which it stops, because the
// density term asymptotes to zero but never reaches it. `RedBalloon` ascended at
// a constant ~4.7 m/s for as long as the process ran, and `escape.rs` is
// deliberately blind upward (a lander under thrust is legitimately unbounded), so
// nothing ever reported it.
//
// Both environmental facts now arrive as inputs:
//
//   * `rho0`    — ambient density at datum. DEFAULT 0: a body with no declared
//                 atmosphere is in vacuum, which is the correct reading of "the
//                 Moon" and makes buoyancy and drag identically zero. An Earth-like
//                 scene authors 1.225 and gets the old behaviour back.
//   * `gravity` — local gravity, wired from the environment's `gravity_accel`
//                 output (see `vessels/balloons/modelica_balloon.usda`). Buoyancy
//                 is a weight difference, so it must use the SAME g the weight
//                 does; a scene cannot make those two disagree.
//
// The atmosphere profile itself is unchanged, only rescaled: with rho = P/(R·T)
// and P = P0·(1 − L·h/T0)^5.255, dividing through by rho0 = P0/(R·T0) leaves
// rho = rho0 · (T0/T) · (1 − L·h/T0)^5.255. Same curve, with the sea-level value
// factored out where a scene can state it.
//
// RUMOCA RULES: branch-free equations — `max`/`min` clamps only, no `if`/`when`.
// The clamps are also what keep the profile finite: above ~44 km the pressure
// base goes negative and a fractional power of it is NaN, which would reach Avian
// as a NaN force. `max(…, 0)` makes the model return zero density up there
// instead of poisoning the solver.
model Balloon
  // Note: balloon mass lives on the Avian RigidBody entity as `Mass`.
  // Modelica no longer subtracts weight from netForce — Avian's gravity
  // system applies `F = -m*g` as a separate force. Keep the Avian Mass
  // value in sync with `mass` here if you tune it.
  parameter Real mass = 4.5 "Reference balloon mass kg (matches Avian Mass)";
  // Max gas volume: slightly larger than sphere mesh (r=1m → V≈4.19 m³)
  parameter Real maxVolume = 6.0 "Maximum gas volume m³";
  // Standard sphere drag coefficient
  parameter Real dragCoeff = 0.47 "Sphere drag coefficient";
  // Slow thermal response — volume changes over ~3 s
  parameter Real tau = 3.0 "Volume thermal response time constant s";
  // Initial volume matches sphere mesh (r=1m → V≈4.19 m³)
  parameter Real initVolume = 4.0 "Initial gas volume m³";
  // Datum temperature and lapse rate of the profile `rho0` is quoted at.
  parameter Real t0 = 288.15 "Datum temperature K";
  parameter Real lapse = 0.0065 "Temperature lapse rate K/m";

  // Environment — supplied by the scene, never assumed.
  // The default Sandbox scene is on the Moon, so it leaves `rho0` at zero:
  // no ambient medium means no buoyancy. An Earth demo must author `rho0`.
  input Real rho0 = 0.0 "Ambient density at datum kg/m³ (0 = vacuum)";
  input Real gravity = 1.62 "Local gravity m/s² (wired from the environment)";

  // Inputs from Avian physics (real-time feedback)
  input Real height = 0 "Altitude m from Avian position.y";
  input Real velocity = 0 "Vertical velocity m/s from Avian";

  // State variable (gives Modelica something to integrate)
  Real volume(start = initVolume) "Gas volume m³ with thermal lag";

  // Derived values (algebraic) — declared as outputs so rumoca preserves
  // them in the solver index instead of substituting them away.
  output Real temperature "Ambient temperature K (profile at this altitude)";
  output Real airDensity "Ambient density kg/m³";
  output Real buoyancy "Buoyancy force N = rho * V * g";
  output Real drag "Drag force N opposing motion";
  output Real netForce "External force N from balloon physics = buoyancy - drag (gravity applied by Avian)";

equation
  // Profile temperature. Floored at 1 K so it can never divide to infinity —
  // the linear lapse would otherwise pass through zero at ~44 km.
  temperature = max(t0 - lapse * height, 1.0);

  // Density from the datum value: rho = rho0 * (T0/T) * (pressure ratio).
  // The pressure base is clamped at 0 so the fractional power stays real.
  airDensity = rho0 * (t0 / temperature)
             * max(1.0 - lapse * height / t0, 0.0) ^ 5.255;

  // Volume dynamics — thermal lag (first-order response)
  tau * der(volume) + volume = maxVolume * (temperature / t0);

  // Buoyancy (Archimedes' principle) — the displaced medium's weight, at the
  // SAME local gravity Avian applies to the body.
  buoyancy = airDensity * volume * gravity;

  // Drag: F = 0.5 * rho * Cd * A * v^2, cross-section A = pi * r^2
  // Sphere radius from volume: r = cbrt(3*V / (4*pi))
  // Using volume^(2/3) as proxy for A (proportional to r^2).
  // Sign: drag opposes velocity direction.
  drag = 0.5 * airDensity * dragCoeff * (3.14159 * volume ^ (2.0 / 3.0))
         * velocity * abs(velocity);

  // Net external force routed to Avian. Gravity (weight) is applied by
  // Avian's gravity system separately — we export only the aerodynamic
  // contribution (lift minus drag). In vacuum both terms are zero and the
  // balloon is a 4.5 kg sphere that falls, which is the honest lunar answer.
  netForce = buoyancy - drag;
end Balloon;
