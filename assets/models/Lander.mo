// tagline: Lander — internal GNC + actuator; yields to its pilot via the wired `piloted` authority signal
model Lander
  "Powered-descent lander. The GNC control LAW (Modelica) computes `gnc_throttle`
   directly; the actuator turns the SELECTED command into world force/torque. The
   authority gate is the WIRED `piloted` port (1 when any session — a user or an
   autopilot — possesses the vessel, derived from the possession registry): it
   selects the session's stick when piloted, the internal GNC when not. Because
   `piloted` is wired it is a live input the solver reads — no in-model flag, no
   per-tick scripting, no wire fighting the pilot. The session throttle is
   spool-filtered (feel); the GNC path is DIRECT (responsive braking — a spool lag
   on the GNC is what made an earlier build tumble)."

  // ── Structural parameters ──
  parameter Real max_thrust = 60000.0 "Max engine thrust (N)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s)";
  parameter Real spool_tau = 0.35 "Human-stick spool-lag time constant (s) — pilot feel";

  // ── GNC gains (wire mass/inertia for USD-derived values; der-fed gains are LIVE) ──
  input Real vehicle_mass = 2000.0 "Vehicle mass (kg) — wired from body `mass`";
  input Real kv = 1.2 "Descent-rate tracking gain — LIVE (Inspector-editable)";
  input Real rest_altitude = 1.5 "Altimeter range (m) at leg contact — hover target";
  input Real descent_slope = 0.6 "Descent-speed schedule slope (m/s per m above rest)";
  input Real vy_max = 6.0 "Max commanded descent speed (m/s)";
  input Real ang_authority = 2.0 "Attitude authority (rad/s^2 per unit stick) — LIVE (Inspector-editable)";
  input Real inertia_xx = 6250.0 "Body inertia about X — wired from body";
  input Real inertia_yy = 6250.0 "Body inertia about Y — wired from body";
  input Real inertia_zz = 6250.0 "Body inertia about Z — wired from body";
  input Real g = 1.62 "Local gravity (m/s^2)";

  // ── Sensor feedback (wired → live) ──
  input Real altitude = 60.0 "Altimeter range (m)";
  input Real descent_rate = 0.0 "Body vertical velocity (m/s)";
  input Real q_w = 1.0; input Real q_x = 0.0; input Real q_y = 0.0; input Real q_z = 0.0;

  // ── Authority + command inputs ──
  input Real piloted = 0.0 "1 = a session (User/Autopilot) drives; 0 = intrinsic GNC. WIRED from the vessel's `piloted` port (derived from possession) — a first-class control-authority signal, not an in-model flag";
  input Real external_throttle = 0.0 "Session vertical thrust command 0..1";
  input Real pitch = 0.0; input Real roll = 0.0; input Real yaw = 0.0;

  // ── Outputs ──
  output Real force_x; output Real force_y; output Real force_z;
  output Real torque_x; output Real torque_y; output Real torque_z;
  output Real throttle "Effective throttle fraction 0..1 (telemetry / flame)";

  Real m_prop(start = 2000.0);
  Real thrust;
  Real vy_sched, target_vy, a_cmd, gnc_raw, gnc_pos, gnc_thrust, gnc_throttle;
  Real cmd_throttle;
  // LIVE (der-fed) copies of the tunable gains — a `der` stops rumoca folding them.
  // (`piloted` needs no such trick: it's WIRED, hence already a live input.)
  Real kv_live(start = 1.2);
  Real ang_live(start = 2.0);
  Real filter_throttle(start = 0.0), filter_pitch(start = 0.0), filter_roll(start = 0.0), filter_yaw(start = 0.0);
  Real f_loc_y, t_loc_x, t_loc_y, t_loc_z;
  Real f_world_x, f_world_y, f_world_z, t_world_x, t_world_y, t_world_z;

equation
  // Keep the tunable gains LIVE (der-fed → not folded).
  der(kv_live) = (kv - kv_live) / 0.02;
  der(ang_live) = (ang_authority - ang_live) / 0.02;

  // ── GNC LAW → gnc_throttle (DIRECT, no spool). Velocity-scheduled descent. ──
  vy_sched = min(max(descent_slope * (altitude - rest_altitude), 0.0), vy_max);
  target_vy = -vy_sched;
  a_cmd = g + kv_live * (target_vy - descent_rate);
  gnc_raw = vehicle_mass * a_cmd;
  gnc_pos = max(gnc_raw, 0.0);
  gnc_thrust = min(gnc_pos, max_thrust);
  gnc_throttle = gnc_thrust / max_thrust;

  // Human stick is spool-filtered (feel); der keeps external_throttle/pitch/... LIVE.
  der(filter_throttle) = (external_throttle - filter_throttle) / spool_tau;
  der(filter_pitch) = (pitch - filter_pitch) / spool_tau;
  der(filter_roll) = (roll - filter_roll) / spool_tau;
  der(filter_yaw) = (yaw - filter_yaw) / spool_tau;

  // ── AUTHORITY GATE: branch-free select (rumoca-safe). `piloted` (WIRED from the
  //    vessel's possession-derived port) is 1 when a user drives, 0 → intrinsic GNC.
  //    Attitude is user-only (gated), so the autonomous descent issues no torque. ──
  cmd_throttle = piloted * filter_throttle + (1.0 - piloted) * gnc_throttle;
  f_loc_y = cmd_throttle * max_thrust;
  t_loc_x = piloted * filter_pitch * inertia_xx * ang_live;   // τ = I·α, pitch about X
  t_loc_y = piloted * filter_yaw * inertia_yy * ang_live;     // τ = I·α, yaw   about Y
  t_loc_z = piloted * filter_roll * inertia_zz * ang_live;    // τ = I·α, roll  about Z

  // Rotate local +Y thrust into world by the body quaternion.
  f_world_x = 2.0 * (q_x*q_y + q_w*q_z) * f_loc_y;
  f_world_y = (1.0 - 2.0*(q_x*q_x + q_z*q_z)) * f_loc_y;
  f_world_z = 2.0 * (q_y*q_z - q_w*q_x) * f_loc_y;
  t_world_x = t_loc_x + 2.0 * (q_y * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z) - q_z * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y));
  t_world_y = t_loc_y + 2.0 * (q_z * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x) - q_x * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z));
  t_world_z = t_loc_z + 2.0 * (q_x * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y) - q_y * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x));

  force_x = f_world_x; force_y = f_world_y; force_z = f_world_z;
  torque_x = t_world_x; torque_y = t_world_y; torque_z = t_world_z;

  thrust = sqrt(force_x*force_x + force_y*force_y + force_z*force_z);
  der(m_prop) = -thrust / v_e;
  throttle = thrust / max_thrust;
end Lander;
