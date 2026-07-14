// tagline: Lander — the airframe's flight control system: it flies what it is told, and nothing else
model Lander
  "Powered-descent lander FCS. Turns a COMMAND into world force and torque: it
   rotates body thrust by the attitude quaternion, converts stick deflection into
   torque about the body axes, and burns propellant for what it produces. It holds
   no guidance law of its own — there is no altitude in here, and no setpoint.

   Two command sources, arbitrated by the wired `piloted` port (1 when any session —
   a human or an autopilot — possesses the vessel, derived from the possession
   registry). When piloted, the session's stick flies it; when not, `guidance_throttle`
   does, which is an INPUT: a guidance program wires it, and an airframe with nothing
   wired into it commands zero thrust and falls.

   That is the whole point. A lander is a vehicle, not a mission: spawn one and it
   sits there until somebody — a pilot or a guidance program the scene composed in —
   tells it to burn."

  // ── Structural parameters ──
  parameter Real max_thrust = 60000.0 "Max engine thrust (N)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s)";
  parameter Real spool_tau = 0.35 "Human-stick spool-lag time constant (s) — pilot feel";

  // ── Body properties (wired from the rigid body) ──
  input Real inertia_xx = 6250.0 "Body inertia about X — wired from body";
  input Real inertia_yy = 6250.0 "Body inertia about Y — wired from body";
  input Real inertia_zz = 6250.0 "Body inertia about Z — wired from body";
  input Real ang_authority = 2.0 "Attitude authority (rad/s^2 per unit stick)";
  input Real q_w = 1.0; input Real q_x = 0.0; input Real q_y = 0.0; input Real q_z = 0.0;

  // ── Authority + command inputs ──
  input Real piloted = 0.0 "1 = a session (human or autopilot) drives; 0 = the guidance wire does. WIRED from the vessel's `piloted` port";
  input Real external_throttle = 0.0 "Session vertical thrust command 0..1";
  input Real pitch = 0.0; input Real roll = 0.0; input Real yaw = 0.0;
  input Real guidance_throttle = 0.0 "Autonomous thrust command 0..1, wired from a guidance program. UNWIRED = 0 = an airframe that does not fly itself";

  // ── Outputs ──
  output Real force_x; output Real force_y; output Real force_z;
  output Real torque_x; output Real torque_y; output Real torque_z;
  output Real throttle "Effective throttle fraction 0..1 (telemetry / flame)";

  Real m_prop(start = 2000.0);
  Real thrust;
  Real cmd_throttle;
  // LIVE (der-fed) copy of the tunable gain — a `der` stops rumoca folding it.
  // (`piloted` needs no such trick: it's WIRED, hence already a live input.)
  Real ang_live(start = 2.0);
  Real filter_throttle(start = 0.0), filter_pitch(start = 0.0), filter_roll(start = 0.0), filter_yaw(start = 0.0);
  Real f_loc_y, t_loc_x, t_loc_y, t_loc_z;
  Real f_world_x, f_world_y, f_world_z, t_world_x, t_world_y, t_world_z;

equation
  // Keep the tunable gain LIVE (der-fed → not folded).
  der(ang_live) = (ang_authority - ang_live) / 0.02;

  // Human stick is spool-filtered (feel); der keeps external_throttle/pitch/... LIVE.
  der(filter_throttle) = (external_throttle - filter_throttle) / spool_tau;
  der(filter_pitch) = (pitch - filter_pitch) / spool_tau;
  der(filter_roll) = (roll - filter_roll) / spool_tau;
  der(filter_yaw) = (yaw - filter_yaw) / spool_tau;

  // ── AUTHORITY GATE: branch-free select (rumoca-safe). `piloted` (WIRED from the
  //    vessel's possession-derived port) is 1 when a session drives, 0 → the guidance
  //    wire. The guidance path is DIRECT (no spool): a spool lag on an automatic
  //    braking command is what made an earlier build tumble. Attitude is session-only,
  //    so an autonomous descent issues no torque. ──
  cmd_throttle = piloted * filter_throttle + (1.0 - piloted) * guidance_throttle;
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
