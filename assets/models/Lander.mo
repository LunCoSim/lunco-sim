// tagline: Powered descent lander — closed-loop throttle from altitude + descent rate
model Lander
  "Soft-landing controller + engine. Reads altitude & descent rate from the
   physics body, commands body-frame thrust to arrest the fall and hover at a
   target height. Gravity stays on the Avian side (do not re-apply it here)."

  // ── Vehicle / engine design parameters ──
  parameter Real vehicle_mass = 1500.0 "Wet vehicle mass (kg) — keep == USD physics:mass";
  parameter Real g_ff = 9.81 "Gravity feed-forward (m/s^2) — sandbox default; integral trims any mismatch";
  parameter Real max_thrust = 30000.0 "Max engine thrust (N) — TWR ~2 at g_ff";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s) — for propellant bookkeeping";

  // ── Guidance set-point + gains ──
  parameter Real target_altitude = 6.0 "Hover / touchdown set-point (m)";
  parameter Real kp = 1.2 "Altitude proportional gain";
  parameter Real kd = 2.5 "Descent-rate (damping) gain";
  parameter Real ki = 0.4 "Integral gain — self-trims to the true mass*g hover point";

  // ── Runtime inputs (wired from the Avian body) ──
  input Real altitude = target_altitude "Body height above ground (m) — from `height` port";
  input Real descent_rate = 0.0 "Vertical velocity (m/s, +up) — from `velocity_y` port";

  // ── State ──
  Real i_err(start=0) "Integral of altitude error — cancels steady gravity bias";
  Real m_prop(start=2000.0) "Propellant remaining (kg) — bookkeeping only (mass is static until G3)";

  // ── Observables / outputs ──
  // Inline conditionals (no Boolean intermediates) per the rumoca
  // algebraic-elimination constraint — see RocketEngine.mo.
  Real a_cmd "Commanded vertical accel (m/s^2)";
  Real thrust "Commanded thrust (N) — wire to `force_local_y`";
  Real throttle "Throttle fraction 0..1 (observable)";

equation
  // PID hover/descent law. g_ff is feed-forward; the integral term absorbs any
  // mismatch between g_ff and the world's actual gravity*mass, so the lander
  // settles to a true hover regardless of the configured gravity constant.
  der(i_err) = target_altitude - altitude;
  a_cmd = g_ff + kp * (target_altitude - altitude) - kd * descent_rate + ki * i_err;

  // thrust = clamp(vehicle_mass * a_cmd, 0, max_thrust)
  thrust = if vehicle_mass * a_cmd < 0.0 then 0.0
           else if vehicle_mass * a_cmd > max_thrust then max_thrust
           else vehicle_mass * a_cmd;
  throttle = thrust / max_thrust;

  // Propellant bookkeeping (not yet fed back to Avian mass — see gap G3).
  der(m_prop) = -thrust / v_e;
end Lander;
