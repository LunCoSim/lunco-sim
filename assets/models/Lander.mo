// tagline: Lander GNC — live-tunable PID hover + manual override, gravity from environment
model Lander
  "Powered-descent guidance + engine for the surface-mission lander. A PID holds
   a target altitude from the body's altitude + descent rate; GRAVITY IS READ FROM
   THE ENVIRONMENT (wire `gravity_accel:g`) — never hardcode 9.81, lunar g ≈ 1.62.
   Gains and set-point are INPUTS so they retune live through the co-sim port
   (no recompile — see doc 34, Decision 2). A manual override lets a possessing
   player fly on thrust directly (hold Space). Do NOT re-apply gravity here — Avian
   owns it. Engine thrust is body-frame: wire `thrust:force_local_y`."

  // ── Engine design parameters (compile-time) ──
  parameter Real vehicle_mass = 2000.0 "Vehicle mass (kg) — keep == USD physics:mass";
  parameter Real max_thrust = 60000.0 "Max engine thrust (N) — TWR ≈ 3 at lunar g";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s) — propellant bookkeeping";
  parameter Real i_band = 5.0 "Anti-windup: only integrate within ±band (m) of the set-point";

  // ── Live-tunable control inputs (set via co-sim port — no restart) ──
  input Real target_altitude = 3.0 "Hover / touchdown set-point (m)";
  input Real kp = 1.2 "Altitude proportional gain";
  input Real kd = 2.5 "Descent-rate (damping) gain";
  input Real ki = 0.4 "Integral gain — trims the steady gravity bias to a true hover";
  input Real engine_enable = 1.0 "1 = PID engine armed, 0 = cut (script cuts after touchdown)";
  input Real manual = 0.0 "1 = manual override active (player flying)";
  input Real manual_throttle = 0.0 "Manual throttle 0..1 while manual = 1";

  // ── Wired-from-body / environment inputs ──
  input Real altitude = target_altitude "Body height above ground (m) — wire `height:altitude`";
  input Real descent_rate = 0.0 "Vertical velocity (m/s, +up) — wire `velocity_y:descent_rate`";
  input Real g = 1.62 "Local gravity (m/s^2) — wire `gravity_accel:g` (env-correct, lunar default)";

  // ── State ──
  Real i_err(start = 0) "Anti-windup integral of altitude error";
  Real m_prop(start = 2000.0) "Propellant remaining (kg) — bookkeeping (mass static until G3)";

  // ── Observables / outputs ──
  // Inline conditionals (no Boolean intermediates) per the rumoca
  // algebraic-elimination constraint — see RocketEngine.mo.
  Real a_cmd "Commanded vertical accel (m/s^2)";
  Real pid_raw "Unclamped PID thrust = vehicle_mass * a_cmd (N)";
  Real pid_pos "Lower-clamped (>=0) PID thrust (N)";
  Real pid_thrust "PID thrust after clamp, before mode select (N)";
  Real thrust "Commanded thrust (N) — wire to `force_local_y`";
  Real throttle "Effective throttle fraction 0..1 (observable)";

equation
  // Anti-windup: only integrate when the engine is armed (auto) AND the body is
  // within ±i_band of the set-point. This stops the 30 m descent set-point error
  // from winding the integral to garbage during the free-fall/braking phase, so
  // the lander still settles to a clean hover once it arrives.
  der(i_err) = if engine_enable > 0.5 and (altitude - target_altitude) < i_band and (altitude - target_altitude) > (-i_band)
               then target_altitude - altitude
               else 0.0;

  // PID hover/descent law. `g` is the feed-forward (from the environment); the
  // integral absorbs any residual mass*g mismatch so it settles to a true hover.
  a_cmd = g + kp * (target_altitude - altitude) - kd * descent_rate + ki * i_err;

  // clamp(vehicle_mass * a_cmd, 0, max_thrust). NOTE: a single `if a > 0 then a
  // else 0` on an ALGEBRAIC variable mis-lowers in rumoca — the relation is
  // evaluated once at init (a = 0 → false → else) and never re-fires, so the
  // clamp is stuck at 0 and the engine never thrusts. Use the `max`/`min`
  // builtins instead (continuous, no zero-crossing event to mishandle).
  pid_raw = vehicle_mass * a_cmd;
  pid_pos = max(pid_raw, 0.0);              // lower clamp at 0
  pid_thrust = min(pid_pos, max_thrust);    // upper clamp at max

  // Mode select as ARITHMETIC gates (manual / engine_enable are 0/1 inputs):
  //   manual=1            -> manual_throttle * max_thrust
  //   manual=0, enable=1  -> pid_thrust
  //   manual=0, enable=0  -> 0
  // A nested `if manual>0.5 ... else if engine_enable>0.5 ...` chain is mis-lowered
  // by rumoca's reconstructor (the manual branch read as 0 at runtime even with
  // manual=1, while pid_thrust computed fine) — so blend continuously instead, the
  // form the algebraic-elimination reconstructor evaluates correctly.
  thrust = manual * (manual_throttle * max_thrust)
           + (1.0 - manual) * engine_enable * pid_thrust;
  throttle = thrust / max_thrust;

  // Propellant bookkeeping (not yet fed back to Avian mass — see gap G3).
  der(m_prop) = -thrust / v_e;
end Lander;
