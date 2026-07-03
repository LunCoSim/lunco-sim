// tagline: Lander GNC — velocity-scheduled powered-descent autopilot (Modelica) + 1-bit manual-override authority gate
model Lander
  "Powered-descent GNC. The low-level control LAW lives here (Modelica): a
   velocity-scheduled vertical autopilot that flies the lander down and eases to a
   hover at leg contact. A 1-bit `manual` gate implements the control-authority
   floor: manual=0 → the GNC drives (baseline, LOWEST tier); when a higher tier
   (autopilot / human) holds the vessel the possession+rhai policy layer sets
   manual=1 and the model actuates the external command instead. Selection is the
   branch-free arithmetic gate below — ALL control math stays here; who-wins is
   decided UPSTREAM (possession arbiter + rhai policy), never computed in rhai/USD."

  // ── Structural parameters (fixed at init from `lunco:params`; change = re-init/reload) ──
  parameter Real max_thrust = 60000.0 "Max engine thrust (N)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s)";

  // ── Live-tunable gains & authorities ──────────────────────────────────────
  // INPUTS, so they are editable AT SIM-RATE with no relaunch — Inspector sliders
  // during an Interactive run, `set(lander, "kv", …)` from rhai/REPL, or the HTTP
  // API / MCP `set_input`. Their init comes from `lunco:params`; each may also be
  // simWired from a USD-derived port (e.g. `mass`, `inertia_*`) instead of a magic
  // constant — so tuning is realtime and provenance is the USD model.
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg) — wire from the body `mass` port";
  input Real kv = 1.2 "Descent-rate tracking gain (P on velocity error)";
  input Real rest_altitude = 1.5 "Altimeter range (m) at leg contact (~1.7) — the hover target; sit just below it";
  input Real descent_slope = 0.6 "Descent-speed schedule slope (m/s of descent per metre above rest)";
  input Real vy_max = 6.0 "Max commanded descent speed (m/s)";
  input Real spool_tau = 0.4 "Command spool-lag time constant (s)";
  // Attitude authority as a target ANGULAR ACCELERATION per unit stick (rad/s^2);
  // the commanded torque is τ = I·α using the body's SENSED inertia (`inertia_*`,
  // wired from the Avian body). Control effort thus scales with the real mass
  // distribution (USD-derived), never a magic N*m constant.
  input Real ang_authority = 7.0 "Commanded angular acceleration per unit stick (rad/s^2)";
  input Real inertia_xx = 6250.0 "Body moment of inertia about X (kg*m^2) — wired from body `inertia_xx`";
  input Real inertia_yy = 6250.0 "Body moment of inertia about Y (kg*m^2) — wired from body `inertia_yy`";
  input Real inertia_zz = 6250.0 "Body moment of inertia about Z (kg*m^2) — wired from body `inertia_zz`";

  // ── Authority gate (WRITTEN by the possession/rhai policy layer — NOT computed here) ──
  input Real manual = 0.0 "0 = GNC autonomous (baseline authority); 1 = obey external command (override)";
  input Real engine_enable = 1.0 "1 = engine armed, 0 = cut";

  // ── Feedback (wired from the Avian body via lunco:simWires) ──
  input Real altitude = 60.0 "Body-centre world height (m) — from body `height`";
  input Real descent_rate = 0.0 "Body vertical velocity (m/s) — from body `velocity_y` (negative = falling)";
  input Real g = 1.62 "Local gravity (m/s^2) — lunar; hover feed-forward";
  input Real q_w = 1.0 "Current orientation quat w";
  input Real q_x = 0.0 "Current orientation quat x";
  input Real q_y = 0.0 "Current orientation quat y";
  input Real q_z = 0.0 "Current orientation quat z";

  // ── External command inputs (used only when manual=1 — the higher-tier pilot) ──
  input Real external_throttle = 0.0 "Vertical thrust command 0..1";
  input Real pitch = 0.0 "Pitch rate command -1..1";
  input Real roll = 0.0 "Roll rate command -1..1";
  input Real yaw = 0.0 "Yaw rate command -1..1";

  // ── Propellant State ──
  Real m_prop(start = 2000.0) "Propellant remaining (kg)";
  Real thrust "Total magnitude of thrust force";

  // ── Outputs (wired to Avian world forces & torques) ──
  output Real force_x "Commanded world X force (N)";
  output Real force_y "Commanded world Y force (N)";
  output Real force_z "Commanded world Z force (N)";
  output Real torque_x "Commanded world X torque (N*m)";
  output Real torque_y "Commanded world Y torque (N*m)";
  output Real torque_z "Commanded world Z torque (N*m)";
  output Real throttle "Effective throttle fraction 0..1 (telemetry / flame)";

  // ── GNC internals ──
  Real vy_sched "Scheduled descent speed (m/s, >=0)";
  Real target_vy "Commanded vertical velocity (m/s, <=0)";
  Real a_cmd "Commanded vertical acceleration (m/s^2)";
  Real gnc_raw, gnc_pos, gnc_thrust "GNC thrust (N), pre/post clamp";
  Real gnc_throttle "GNC commanded throttle fraction 0..1";
  Real cmd_throttle "Selected throttle after the authority gate";

  // ── Manual spool lag (pilot feel; the GNC path is direct) ──
  Real filter_throttle(start = 0.0);
  Real filter_pitch(start = 0.0);
  Real filter_roll(start = 0.0);
  Real filter_yaw(start = 0.0);

  // Local + world force/torque vectors
  Real f_loc_y, t_loc_x, t_loc_y, t_loc_z;
  Real f_world_x, f_world_y, f_world_z, t_world_x, t_world_y, t_world_z;

equation
  // ── GNC LAW: velocity-scheduled powered descent (ALL control math is here) ──
  // Descend fast up high, ease to a hover at leg contact. `g` is the hover
  // feed-forward; the P-term on the descent-rate error does the braking. `min`/`max`
  // (NOT `if` on algebraic vars — rumoca mis-lowers those) keep the clamps continuous.
  vy_sched = min(max(descent_slope * (altitude - rest_altitude), 0.0), vy_max);
  target_vy = -vy_sched;
  a_cmd = g + kv * (target_vy - descent_rate);
  gnc_raw = vehicle_mass * a_cmd;
  gnc_pos = max(gnc_raw, 0.0);              // no negative thrust
  gnc_thrust = min(gnc_pos, max_thrust);    // clamp to engine limit
  gnc_throttle = gnc_thrust / max_thrust;

  // ── AUTHORITY GATE: branch-free select. `manual` is a 0/1 flag from the
  //    possession/rhai layer; GNC is the baseline (lowest tier). A continuous blend
  //    (not a nested `if`) is the form rumoca's reconstructor evaluates correctly. ──
  cmd_throttle = manual * filter_throttle + (1.0 - manual) * gnc_throttle;

  // Spool lag on the MANUAL stick only (time constant `spool_tau`).
  der(filter_throttle) = (external_throttle - filter_throttle) / spool_tau;
  der(filter_pitch) = (pitch - filter_pitch) / spool_tau;
  der(filter_roll) = (roll - filter_roll) / spool_tau;
  der(filter_yaw) = (yaw - filter_yaw) / spool_tau;

  // Local command force (body +Y) & torque. Attitude is manual-only (the vertical
  // GNC issues no torque); gate torque by `manual` so autonomous descent stays hands-off.
  f_loc_y = cmd_throttle * max_thrust;
  t_loc_x = manual * filter_pitch * inertia_xx * ang_authority;   // τ = I·α, pitch about X
  t_loc_y = manual * filter_yaw * inertia_yy * ang_authority;     // τ = I·α, yaw   about Y
  t_loc_z = manual * filter_roll * inertia_zz * ang_authority;    // τ = I·α, roll  about Z

  // Rotate the local +Y thrust into the world frame by the body quaternion.
  f_world_x = 2.0 * (q_x*q_y + q_w*q_z) * f_loc_y;
  f_world_y = (1.0 - 2.0*(q_x*q_x + q_z*q_z)) * f_loc_y;
  f_world_z = 2.0 * (q_y*q_z - q_w*q_x) * f_loc_y;

  t_world_x = t_loc_x + 2.0 * (q_y * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z) - q_z * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y));
  t_world_y = t_loc_y + 2.0 * (q_z * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x) - q_x * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z));
  t_world_z = t_loc_z + 2.0 * (q_x * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y) - q_y * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x));

  // Actuate the commanded force/torque, gated by engine_enable.
  force_x = engine_enable * f_world_x;
  force_y = engine_enable * f_world_y;
  force_z = engine_enable * f_world_z;
  torque_x = engine_enable * t_world_x;
  torque_y = engine_enable * t_world_y;
  torque_z = engine_enable * t_world_z;

  // Propellant bookkeeping + telemetry throttle.
  thrust = sqrt(force_x*force_x + force_y*force_y + force_z*force_z);
  der(m_prop) = -thrust / v_e;
  throttle = thrust / max_thrust;
end Lander;
