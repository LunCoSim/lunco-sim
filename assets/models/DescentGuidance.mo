// tagline: DescentGuidance — the autopilot: it decides how hard to burn, and it is not part of the airframe
model DescentGuidance
  "Velocity-scheduled powered-descent guidance. Reads what a lander SENSES — range to
   the surface and vertical speed — and commands a throttle that tracks a descent
   speed scheduled on altitude: fast while high, slowing to a hover as the legs come
   down. It touches no physics; its one output is a number between 0 and 1.

   This is a MISSION concern, not a vehicle one, and it lives in its own file for
   exactly that reason. It arrives on a scene's lander through a `references` arc, and
   a scene that does not compose it gets an airframe that does not fly itself. Delete
   the program prim and the lander has no autopilot.

   It yields to the pilot the moment one takes the stick: `piloted` is wired from the
   possession registry, and it zeroes the command. The airframe's own gate would
   ignore this output anyway; zeroing it here means the autopilot also stops burning
   propellant it is no longer flying with."

  // ── Schedule + gains ──
  input Real kv = 1.2 "Descent-rate tracking gain";
  input Real rest_altitude = 1.5 "Altimeter range (m) at leg contact — the hover target";
  input Real descent_slope = 0.6 "Descent-speed schedule slope (m/s per m above rest)";
  input Real vy_max = 6.0 "Max commanded descent speed (m/s)";
  input Real g = 1.62 "Local gravity (m/s^2)";
  input Real max_thrust = 60000.0 "The airframe's max engine thrust (N) — what the command is a fraction OF";

  // ── Sensor feedback (wired → live) ──
  input Real altitude = 60.0 "Altimeter range (m)";
  input Real descent_rate = 0.0 "Body vertical velocity (m/s)";
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg) — wired from the body";

  // ── Authority ──
  input Real piloted = 0.0 "1 = a session holds the stick. WIRED from possession; the autopilot stands down";
  input Real engage = 1.0 "1 = fly the descent, 0 = stand by. A script owns this: it is the WHEN of the autopilot, and the when is a mission decision";

  // ── Output ──
  output Real throttle_cmd "Commanded throttle 0..1 → the airframe's `guidance_throttle`";

  Real vy_sched, target_vy, a_cmd, raw, pos, cmd_thrust, law;
  // LIVE (der-fed) copy of the tunable gain — a `der` stops rumoca folding it.
  Real kv_live(start = 1.2);

equation
  der(kv_live) = (kv - kv_live) / 0.02;

  // Velocity-scheduled descent: command a descent speed proportional to height above
  // the resting altitude (capped), then a thrust that tracks it. DIRECT — no spool.
  vy_sched = min(max(descent_slope * (altitude - rest_altitude), 0.0), vy_max);
  target_vy = -vy_sched;
  a_cmd = g + kv_live * (target_vy - descent_rate);
  raw = vehicle_mass * a_cmd;
  pos = max(raw, 0.0);
  cmd_thrust = min(pos, max_thrust);
  law = cmd_thrust / max_thrust;

  // Branch-free (rumoca-safe): stand down when disengaged OR when a session is flying.
  throttle_cmd = engage * (1.0 - piloted) * law;
end DescentGuidance;
