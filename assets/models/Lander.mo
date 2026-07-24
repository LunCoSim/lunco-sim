// tagline: Lander — the airframe's flight control system: it flies what it is told, and nothing else
model Lander
  "Powered-descent lander FCS. Turns a COMMAND into world force and torque: it
   rotates body thrust by the attitude quaternion, converts stick deflection into
   torque about the body axes, and burns propellant for what it produces. It holds
   no guidance law of its own — there is no altitude in here, and no setpoint.

   Two command sources, arbitrated by the wired `piloted` port (1 when any session —
   a human or an autopilot — possesses the vessel, derived from the possession
   registry). When piloted, the session's stick flies it; when not, the `guidance_*`
   wires do, and they are INPUTS: a guidance program wires them, and an airframe with
   nothing wired into it commands zero thrust and falls.

   `piloted` selects the SETPOINT SOURCE. It is not a permission gate. It used to gate
   attitude torque as well, which meant an unpossessed vehicle had no attitude
   authority at all: any tilt it acquired was permanent, thrust steered it along that
   tilt, and the descent diverged with nothing able to intervene. Throttle was already
   modelled as a source select; attitude now matches it.

   Stabilisation is separate from command, and separate from guidance. `attitude_hold`
   turns on an RCS loop that holds the vehicle upright and damps rotation REGARDLESS of
   who is commanding — the vehicle's own stability augmentation, the equivalent of a
   helicopter's SAS, not a mission law. It is unwired (0) by default, so a bare
   airframe behaves exactly as before and nothing is imposed on scenes that want the
   raw vehicle.

   That is the whole point. A lander is a vehicle, not a mission: spawn one and it
   sits there until somebody — a pilot or a guidance program the scene composed in —
   tells it to burn."

  // ── Structural parameters ──
  parameter Real max_thrust = 60000.0 "Max engine thrust (N)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s)";
  parameter Real spool_tau = 0.35 "Human-stick spool-lag time constant (s) — pilot feel";
  parameter Real low_fuel_mass = 200.0 "Low-fuel event threshold (kg)";
  parameter Real depleted_mass = 0.5 "Propellant-depleted event threshold (kg)";

  // ── Body properties (wired from the rigid body) ──
  input Real inertia_xx = 6250.0 "Body inertia about X — wired from body";
  input Real inertia_yy = 6250.0 "Body inertia about Y — wired from body";
  input Real inertia_zz = 6250.0 "Body inertia about Z — wired from body";
  input Real ang_authority = 0.6 "Attitude authority = angular acceleration (rad/s^2) per unit stick. Vacuum: no aerodynamic damping, so rate ramps while held and holds on release unless `attitude_hold` is wired on.";
  input Real q_w = 1.0; input Real q_x = 0.0; input Real q_y = 0.0; input Real q_z = 0.0;

  // ── Authority + command inputs ──
  input Real piloted = 0.0 "1 = a session (human or autopilot) drives; 0 = the guidance wire does. WIRED from the vessel's `piloted` port";
  input Real external_throttle = 0.0 "Session vertical thrust command 0..1";
  input Real pitch = 0.0; input Real roll = 0.0; input Real yaw = 0.0;
  input Real guidance_throttle = 0.0 "Autonomous thrust command 0..1, wired from a guidance program. UNWIRED = 0 = an airframe that does not fly itself";
  input Real guidance_pitch = 0.0 "Autonomous pitch command -1..1 — the attitude twin of `guidance_throttle`";
  input Real guidance_roll = 0.0 "Autonomous roll command -1..1";
  input Real guidance_yaw = 0.0 "Autonomous yaw command -1..1";

  // ── Body angular rate, world frame (wired from the rigid body) ──
  input Real omega_x = 0.0 "Angular rate about world X (rad/s) — wired from the body's `angvel_x`";
  input Real omega_y = 0.0 "Angular rate about world Y (rad/s) — wired from the body's `angvel_y`";
  input Real omega_z = 0.0 "Angular rate about world Z (rad/s) — wired from the body's `angvel_z`";
  input Real leg_force_px = 0.0 "Solver reaction at the +X landing leg (N)";
  input Real leg_force_nx = 0.0 "Solver reaction at the -X landing leg (N)";
  input Real leg_force_pz = 0.0 "Solver reaction at the +Z landing leg (N)";
  input Real leg_force_nz = 0.0 "Solver reaction at the -Z landing leg (N)";

  // ── Stability augmentation (RCS) ──
  input Real attitude_hold = 0.0 "1 = the RCS holds the vehicle upright and damps rotation, whoever is commanding. UNWIRED = 0 = bare airframe";
  parameter Real hold_kp = 2.0 "Stabiliser stiffness: angular accel (rad/s^2) per unit tilt sine";
  parameter Real hold_kd = 2.5 "Stabiliser damping: angular accel (rad/s^2) per rad/s. Chosen above 2*sqrt(hold_kp) so recovery is overdamped and never oscillates the shot";

  // ── Outputs ──
  output Real force_x; output Real force_y; output Real force_z;
  output Real torque_x; output Real torque_y; output Real torque_z;
  output Real throttle "Effective throttle fraction 0..1 (telemetry / flame)";
  output Real low_fuel "Discrete 0/1 low-fuel signal";
  output Real depleted "Discrete 0/1 propellant-depleted signal";
  output Real touchdown "Discrete 0/1 touchdown signal from combined leg reactions";

  Real m_prop(start = 2000.0);
  Real thrust;
  Real cmd_throttle;
  // LIVE (der-fed) copy of the tunable gain — a `der` stops rumoca folding it.
  // (`piloted` needs no such trick: it's WIRED, hence already a live input.)
  Real ang_live(start = 0.6);
  Real filter_throttle(start = 0.0), filter_pitch(start = 0.0), filter_roll(start = 0.0), filter_yaw(start = 0.0);
  Real cmd_pitch, cmd_roll, cmd_yaw;
  Real f_loc_y, t_loc_x, t_loc_y, t_loc_z;
  Real f_world_x, f_world_y, f_world_z, t_world_x, t_world_y, t_world_z;
  Real up_x, up_y, up_z;
  Real hold_x, hold_y, hold_z;
  Real total_leg_force;
  LunCo.Logic.AboveThreshold touchdown_check(
    threshold = 250.0,
    transition_width = 100.0);

equation
  // Keep the tunable gain LIVE (der-fed → not folded).
  der(ang_live) = (ang_authority - ang_live) / 0.02;

  // Human stick is spool-filtered (feel); der keeps external_throttle/pitch/... LIVE.
  der(filter_throttle) = (external_throttle - filter_throttle) / spool_tau;
  der(filter_pitch) = (pitch - filter_pitch) / spool_tau;
  der(filter_roll) = (roll - filter_roll) / spool_tau;
  der(filter_yaw) = (yaw - filter_yaw) / spool_tau;

  // ── SOURCE SELECT: branch-free (rumoca-safe). `piloted` (WIRED from the vessel's
  //    possession-derived port) is 1 when a session drives, 0 → the guidance wires.
  //    The guidance path is DIRECT (no spool): a spool lag on an automatic braking
  //    command is what made an earlier build tumble.
  //
  //    Every axis selects the same way. Attitude used to read `piloted * stick`, which
  //    is a PERMISSION gate, not a select — the unpiloted branch had no term at all, so
  //    an autonomous vehicle could not command attitude even when a guidance program
  //    wanted it to. ──
  cmd_throttle = piloted * filter_throttle + (1.0 - piloted) * guidance_throttle;
  cmd_pitch    = piloted * filter_pitch    + (1.0 - piloted) * guidance_pitch;
  cmd_yaw      = piloted * filter_yaw      + (1.0 - piloted) * guidance_yaw;
  cmd_roll     = piloted * filter_roll     + (1.0 - piloted) * guidance_roll;

  f_loc_y = cmd_throttle * max_thrust;
  t_loc_x = cmd_pitch * inertia_xx * ang_live;   // τ = I·α, pitch about X
  t_loc_y = cmd_yaw   * inertia_yy * ang_live;   // τ = I·α, yaw   about Y
  t_loc_z = cmd_roll  * inertia_zz * ang_live;   // τ = I·α, roll  about Z

  // Rotate local +Y thrust into world by the body quaternion. Thrust points along
  // body +Y BY DEFINITION, so it is `up` (computed once, below) scaled by the
  // engine force — never a second transcription of the same rotation. The two
  // used to be written out separately and disagreed with each other, which is
  // precisely the bug this arrangement makes unrepresentable.
  f_world_x = up_x * f_loc_y;
  f_world_y = up_y * f_loc_y;
  f_world_z = up_z * f_loc_y;
  t_world_x = t_loc_x + 2.0 * (q_y * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z) - q_z * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y));
  t_world_y = t_loc_y + 2.0 * (q_z * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x) - q_x * (q_x * t_loc_y - q_y * t_loc_x + q_w * t_loc_z));
  t_world_z = t_loc_z + 2.0 * (q_x * (q_z * t_loc_x - q_x * t_loc_z + q_w * t_loc_y) - q_y * (q_y * t_loc_z - q_z * t_loc_y + q_w * t_loc_x));

  // ── STABILITY AUGMENTATION (world frame) ──
  //
  // Body +Y in world coordinates — the direction thrust actually points. This is the
  // same rotation the force above uses, so "upright" means "thrust opposes gravity",
  // which is the property that matters for a descent.
  // R*(0,1,0) for q = (q_w, q_x, q_y, q_z), i.e. the MIDDLE COLUMN of the
  // body→world rotation matrix. The `q_w` cross terms are +q_w*q_x on Z and
  // -q_w*q_z on X; they were written with both signs flipped, which is the
  // TRANSPOSE — the world→body rotation. Two consequences, both fatal and both
  // invisible while the vehicle is upright (at q = identity the two agree):
  //   1. thrust was rotated by the inverse, so any tilt steered the vehicle to
  //      the mirror-image heading;
  //   2. `up_z` came out with the wrong sign, so the tilt error below fed the
  //      stabiliser BACKWARDS and it drove the lean it was meant to null. The
  //      loop was positive feedback, which is why the divergence compounded.
  // Verified against v' = v + 2*qv x (qv x v + q_w*v) for v = (0,1,0).
  up_x = 2.0 * (q_x*q_y - q_w*q_z);
  up_y = 1.0 - 2.0*(q_x*q_x + q_z*q_z);
  up_z = 2.0 * (q_y*q_z + q_w*q_x);

  // Tilt error as up × ŷ = (-up_z, 0, up_x): a world-frame axis whose magnitude is
  // sin(tilt) and which vanishes exactly when upright. Yaw has no reference to hold —
  // heading is nobody's business but the pilot's — so that axis is damped only.
  //
  // Limitation, stated rather than hidden: sin(tilt) is not monotonic past 90°, so
  // this has a stable equilibrium upright and an unstable one inverted. It recovers
  // any lean; it will not right a vehicle that is already upside down. That is
  // adequate for stabilisation, and a flip is a scene bug, not a flight condition.
  //
  // Torque is I·α throughout, so the gains are angular accelerations and behave the
  // same on any vehicle inertia — the stabiliser does not need retuning per airframe.
  hold_x = attitude_hold * inertia_xx * (hold_kp * (-up_z) - hold_kd * omega_x);
  hold_y = attitude_hold * inertia_yy * (                  - hold_kd * omega_y);
  hold_z = attitude_hold * inertia_zz * (hold_kp * ( up_x) - hold_kd * omega_z);

  force_x = f_world_x; force_y = f_world_y; force_z = f_world_z;
  torque_x = t_world_x + hold_x;
  torque_y = t_world_y + hold_y;
  torque_z = t_world_z + hold_z;

  thrust = sqrt(force_x*force_x + force_y*force_y + force_z*force_z);
  der(m_prop) = -thrust / v_e;
  throttle = thrust / max_thrust;
  low_fuel = max(0.0, min(1.0, 0.5 + 100.0 * (low_fuel_mass - m_prop)));
  depleted = max(0.0, min(1.0, 0.5 + 100.0 * (depleted_mass - m_prop)));
  total_leg_force = leg_force_px + leg_force_nx + leg_force_pz + leg_force_nz;
  touchdown_check.value = total_leg_force;
  touchdown = touchdown_check.active;
end Lander;
