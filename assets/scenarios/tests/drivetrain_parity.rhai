// ── Drivetrain parity test — raycast wheels vs joint wheels ─────────────────
//
// TEST-ONLY. Runs against `scenes/tests/drivetrain_parity.usda`, which puts
// the SAME rover asset on the SAME ground twice, differing in exactly one
// authored opinion: `variants = { string drivetrain = "raycast" | "physical" }`.
// This script drives BOTH from ONE tick loop — same command, same frame — and
// asserts the contracts that the two realizations actually share.
//
// WHAT IT GUARDS. Raycast and joint wheels are two REALIZATIONS of ONE parameter
// set. The parity invariant is structural: both realizations read the same
// authored motor/gearbox reduction, and the raycast drive force includes the
// same torque-speed term as the joint actuator. Both therefore cap at the same
// axle speed for a rover built from the same file.
// Both read the single authored motor/gearbox/shaft network. The Modelica
// equations and wheel/tire load determine the operating point for both
// realizations; no separate Rust motor rule exists.
//
//     v_max = omega_max * r = 12.0 * 0.4 = 4.8 m/s
//
// If that unification is ever undone, this test says so in one line.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// There is NO exit-code mechanism for a scenario — a scenario is a tick hook on
// a live sim, not a process. So this file uses the verdict contract the rhai
// test harness already established (`assets/scripting/tests/lib/test_assert.rhai`):
// checks return "" on pass or a message on failure, and the run ends with a
// greppable verdict on stdout (the process log):
//
//     TESTS_OK <n>                  or   TESTS_FAIL <n>/<m>   (+ "  FAIL ..." lines)
//     DRIVETRAIN PARITY: PASS       or   DRIVETRAIN PARITY: FAIL      <- LAST line
//
// A caller greps for `DRIVETRAIN PARITY:`. The suite ALSO emits
// `DRIVETRAIN_PARITY` on the telemetry bus with value "PASS"/"FAIL" and raises a
// toast, so a UI run is not silent either.
//
// The helpers are defined LOCALLY rather than imported: `tests/lib/*.rhai` is
// concatenated by a shell runner for one-shot `RunRhai` scripts and is NOT part
// of the scenario prelude, and rhai has no cross-file function sharing.
//
// ── WHY A PASSING-LOOKING RUN IS NOT ENOUGH ─────────────────────────────────
//
// Scenarios fail SILENTLY: a hook that never fires, a `find` that returns -1, a
// phase that never advances all look exactly like a clean run. So this file
// treats "no numbers" as a FAILURE, not a pass:
//   * `on_start` prints a banner with the resolved gids; a missing rover fails
//     immediately and loudly instead of ticking forever.
//   * every sample prints t / v / distance for BOTH rovers — the log IS the
//     evidence, and a run with no `[parity]` sample lines proves nothing.
//   * a rover that never reaches half the expected top speed fails as "did not
//     move", so a dead command surface cannot masquerade as parity (0 == 0).
// If you do not see a sample table and a verdict line, the test did not run.

// ── Local assertion helpers (stateless — `this` is NOT bound in helpers) ────

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT (matching the skid-drive sign convention in `prelude/nav.rhai`, where
/// +steer yaws right). Sign is taken from the yaw-plane cross product, so a
/// rover that turned the WRONG way cannot pass by magnitude alone.
/// THE VEHICLE'S MASS PROPERTIES AS THE SOLVER SEES THEM, both realizations.
///
/// Read through the avian cosim ports (`mass`, `com_y`, `inertia_yy`), which
/// return `ComputedMass` / `ComputedCenterOfMass` / `ComputedAngularInertia` —
/// the components the integrator actually uses, not the authored overrides. An
/// override that avian discarded for want of its `NoAuto*` marker reads back
/// UNCHANGED here, which is the whole reason to sample the computed side.
///
/// The physical rover keeps its wheels as four SEPARATE bodies, so its chassis
/// body alone is not the vehicle: mass and centre of mass are aggregated over
/// chassis + suspension carriers + wheels. That aggregate is exactly what
/// `fold_raycast_wheel_mass` folds onto the raycast chassis, whose reduced
/// children are not Avian bodies. Comparing chassis to chassis would compare
/// the full assembly against its root only and prove nothing.
///
/// The carrier is a physical child in the full realization and an explicit
/// `LunCoMassContributionAPI` in the reduced realization, so both must be
/// included here.
///
/// `mount_y` is the authored wheel mount — one number for both realizations now
/// that the carrier is the authored axle frame in both realizations, which is
/// the property under test. The physical child centres sit at their authored
/// carrier origin, so their height in the rover frame IS the mount.
fn mass_props(rc, ph, mount_y) {
    let m_ch = get(ph, "mass");
    let com_ch = get(ph, "com_y");
    if m_ch == () { m_ch = 0.0; }
    if com_ch == () { com_ch = 0.0; }

    let m_w = 0.0;
    let n = 0;
    for w in ["FL", "FR", "RL", "RR"] {
        let carrier = find("/DrivetrainParity/RoverPhysical/Suspension_" + w);
        let wheel = find("/DrivetrainParity/RoverPhysical/Suspension_" + w + "/Wheel_" + w);
        if carrier >= 0 {
            let mc = get(carrier, "mass");
            if mc != () { m_w += mc; }
        }
        if wheel >= 0 {
            let mw = get(wheel, "mass");
            if mw != () { m_w += mw; }
        }
        n += 1;
    }

    let total = m_ch + m_w;
    let com_ph = com_ch;
    if total > 0.0 { com_ph = (m_ch * com_ch + m_w * mount_y) / total; }

    // A missing port reads `()`. Coerce to 0.0 rather than carrying a unit into
    // the arithmetic: 0 kg against the full assembly fails loudly, which is what a rover
    // that never published its mass properties deserves.
    let m_rc = get(rc, "mass");
    let com_rc = get(rc, "com_y");
    let iyy_rc = get(rc, "inertia_yy");
    let iyy_ch = get(ph, "inertia_yy");
    if m_rc == () { m_rc = 0.0; }
    if com_rc == () { com_rc = 0.0; }
    if iyy_rc == () { iyy_rc = 0.0; }
    if iyy_ch == () { iyy_ch = 0.0; }

    return #{
        m_rc: m_rc, m_ph: total, wheels: n,
        com_rc: com_rc, com_ph: com_ph,
        iyy_rc: iyy_rc, iyy_ch: iyy_ch
    };
}

/// One log row. The table is the evidence; keep it readable.
///
/// Built by successive `+=` rather than one concatenation chain: rhai caps
/// EXPRESSION complexity, and a single six-value chain trips
/// "Expression exceeds maximum complexity" at compile time.
fn row(tag, t, v_rc, v_ph, d_rc, d_ph) {
    let s = "[parity] " + tag;
    s += " t=" + r2(t) + "s";
    s += "  v_ray=" + r2(v_rc);
    s += "  v_phy=" + r2(v_ph);
    s += "  d_ray=" + r2(d_rc);
    s += "  d_phy=" + r2(d_ph);
    print(s);
}

/// Read the solved wheel boundary without adding a Rust-only debug API. The
/// actuator endpoints are authored child ports, while the tire/Avian values are
/// the live runtime state. Keeping this probe in the Rhai gate makes the
/// producer-to-consumer mismatch visible at the exact time parity diverges.
// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    // Tunables live on `this`: named fns cannot see top-level `const`s.
    this.t = 0.0;
    this.settle_s = 3.0;    // let both rovers land and stop bouncing
    this.drive_s = 12.0;    // full throttle, straight — long enough to plateau
    this.steer_s = 6.0;     // full throttle + steady steer — yaw authority
    this.plateau_s = 3.0;   // trailing window of the drive phase averaged as v_terminal
    this.sample_s = 0.5;    // log cadence

    // The authored contract under test. omega_max is the axle reduction of the
    // motor/gearbox pair composed by the shared rover.
    this.omega_max = 12.0;
    this.wheel_r = 0.4;
    this.v_ceiling = 4.8;   // omega_max * wheel_r

    this.phase = "settle";
    this.next_sample = 0.0;

    this.rc = find("/DrivetrainParity/RoverRaycast");
    this.ph = find("/DrivetrainParity/RoverPhysical");

    this.d_rc = 0.0;   this.d_ph = 0.0;
    this.p_rc = ();    this.p_ph = ();
    this.vmax_rc = 0.0; this.vmax_ph = 0.0;
    this.vsum_rc = 0.0; this.vsum_ph = 0.0; this.vn = 0;
    this.vterm_rc = 0.0; this.vterm_ph = 0.0;
    // The trailing straight-line window is the high-speed stability gate.  A
    // mean terminal speed alone can hide a rover alternating above and below
    // the target, so retain extrema of both speed and chassis height.
    this.vmin_rc = (); this.vmax_plateau_rc = ();
    this.vmin_ph = (); this.vmax_plateau_ph = ();
    this.ymin_rc = (); this.ymax_rc = ();
    this.ymin_ph = (); this.ymax_ph = ();
    this.f0_rc = ();   this.f0_ph = ();
    // Accumulated swept yaw (degrees) + the previous tick's forward vector the
    // accumulation differences against. Start at 0.0, not `()`: the steer phase
    // SUMS into these.
    this.yaw_rc = 0.0;  this.yaw_ph = 0.0;
    this.fprev_rc = (); this.fprev_ph = ();

    // The authored wheel mount, one number for BOTH realizations — the property
    // this whole file exists to keep true. Sampled mass properties land here once
    // the bodies are alive (avian publishes `Computed*` a few ticks after load).
    this.mount_y = -0.65;
    this.mp = ();
    print("[parity] ── drivetrain parity test ─────────────────────────────");
    print("[parity] raycast rover gid=" + this.rc + "  physical rover gid=" + this.ph);
    print("[parity] contract: omega_max=" + this.omega_max + " rad/s * r=" + this.wheel_r +
          " m = " + this.v_ceiling + " m/s for BOTH realizations");
    print("[parity] phases: settle " + this.settle_s + "s -> straight " + this.drive_s +
          "s -> steer " + this.steer_s + "s");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op: `find` yields -1 and every
    // subsequent read is (). Fail on the FIRST tick instead of ticking forever.
    if this.rc == () || this.ph == () || this.rc < 0 || this.ph < 0 {
        print("  PARITY FAIL: rover prims not found (raycast gid=" + this.rc +
              ", physical gid=" + this.ph + ") - wrong scene? expected " +
              "scenes/tests/drivetrain_parity.usda");
        print("TESTS_FAIL 1/1");
        print("DRIVETRAIN PARITY: FAIL");
        emit("DRIVETRAIN_PARITY", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let t = this.t;

    // Sample the mass properties once, mid-settle: late enough that avian has
    // published `Computed*` and `fold_raycast_wheel_mass` has run, early enough
    // that the run is still standing still and nothing has been driven.
    if this.mp == () && t > 1.0 {
        this.mp = mass_props(this.rc, this.ph, this.mount_y);
    }
    let t_drive_end = this.settle_s + this.drive_s;
    let t_steer_end = t_drive_end + this.steer_s;

    let pos_rc = world_pos(this.rc);
    let pos_ph = world_pos(this.ph);

    // Speed from POSITION DELTA, not `speed()`. `speed()` reports 0.0 for both
    // rovers here even while they visibly travel tens of metres — it reads a
    // velocity source these vessels do not publish, and trusting it produced a
    // "both ~zero, so they match" reading that is exactly the dead-test trap
    // this file is written to refuse. Displacement per tick is measured from
    // the same `world_pos` the odometer uses, so speed and distance can never
    // disagree about whether the rover moved.
    let d_t = dt();
    let prev_rc = this.p_rc;
    let prev_ph = this.p_ph;
    let v_rc = if d_t > 0.0 { seg(prev_rc, pos_rc) / d_t } else { 0.0 };
    let v_ph = if d_t > 0.0 { seg(prev_ph, pos_ph) / d_t } else { 0.0 };
    // Advance the reference EVERY tick, unconditionally, and before any phase
    // block can `return`.
    //
    // It used to be advanced only inside the `drive` block, so during `steer`
    // this divided displacement-since-the-last-drive-tick by ONE tick's `dt`.
    // The steer log read `v_ray=399.41  v_phy=452.74` against an authored
    // ceiling of 4.8 m/s — a rover apparently doing 1400 km/h. Nothing was
    // wrong with the physics; the readout was integrating. A failing test whose
    // evidence lies is worse than no evidence: it invites a hunt for a runaway
    // that never happened, and it hides the real gap sitting next to it.
    this.p_rc = pos_rc;
    this.p_ph = pos_ph;

    // ── Phase 1: settle ─────────────────────────────────────────────────────
    if this.phase == "settle" {
        brake(this.rc);
        brake(this.ph);
        if t >= this.settle_s {
            this.phase = "drive";
            // Reset the odometers here, not at t=0: the drop-and-settle motion
            // is not travel and would be charged to whichever rover bounced more.
            this.p_rc = pos_rc;
            this.p_ph = pos_ph;
            this.d_rc = 0.0;
            this.d_ph = 0.0;
            this.next_sample = t;
            let s = "[parity] settled (v_ray=" + r2(v_rc);
            s += " v_phy=" + r2(v_ph);
            s += ") - FULL THROTTLE, straight, both rovers, same tick";
            print(s);
        }
        return;
    }

    // ── Phase 2: full throttle, straight ────────────────────────────────────
    if this.phase == "drive" {
        drive(this.rc, 1.0, 0.0);
        drive(this.ph, 1.0, 0.0);

        // Distance is the STRAIGHT-RUN odometer, so it accumulates only here.
        // The position REFERENCE is advanced unconditionally at the end of the
        // tick — see the note there.
        this.d_rc += seg(prev_rc, pos_rc);
        this.d_ph += seg(prev_ph, pos_ph);
        if v_rc > this.vmax_rc { this.vmax_rc = v_rc; }
        if v_ph > this.vmax_ph { this.vmax_ph = v_ph; }

        // Terminal speed = mean over the trailing plateau window, not a single
        // frame: one noisy tick is not a top speed, and the two solvers ripple
        // differently around the same mean.
        if t >= t_drive_end - this.plateau_s {
            this.vsum_rc += v_rc;
            this.vsum_ph += v_ph;
            this.vn += 1;
            if this.vmin_rc == () || v_rc < this.vmin_rc { this.vmin_rc = v_rc; }
            if this.vmax_plateau_rc == () || v_rc > this.vmax_plateau_rc { this.vmax_plateau_rc = v_rc; }
            if this.vmin_ph == () || v_ph < this.vmin_ph { this.vmin_ph = v_ph; }
            if this.vmax_plateau_ph == () || v_ph > this.vmax_plateau_ph { this.vmax_plateau_ph = v_ph; }
            if this.ymin_rc == () || pos_rc[1] < this.ymin_rc { this.ymin_rc = pos_rc[1]; }
            if this.ymax_rc == () || pos_rc[1] > this.ymax_rc { this.ymax_rc = pos_rc[1]; }
            if this.ymin_ph == () || pos_ph[1] < this.ymin_ph { this.ymin_ph = pos_ph[1]; }
            if this.ymax_ph == () || pos_ph[1] > this.ymax_ph { this.ymax_ph = pos_ph[1]; }
        }

        if t >= this.next_sample {
            row("drive", t, v_rc, v_ph, this.d_rc, this.d_ph);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_drive_end {
            // Plain statements, not inline `if` EXPRESSIONS: rhai caps expression
            // complexity, and `if cond { a.b / c.to_float() } else { 0.0 }` as a
            // right-hand side trips it (see `row()` for the same limit on
            // concatenation chains).
            this.vterm_rc = 0.0;
            this.vterm_ph = 0.0;
            if this.vn > 0 {
                let n = this.vn.to_float();
                this.vterm_rc = this.vsum_rc / n;
                this.vterm_ph = this.vsum_ph / n;
            }
            this.f0_rc = world_forward(this.rc);
            this.f0_ph = world_forward(this.ph);
            // Seed the yaw accumulator's reference so the first steer tick
            // differences against the heading at the START of the steer phase.
            this.fprev_rc = this.f0_rc;
            this.fprev_ph = this.f0_ph;
            this.phase = "steer";
            // Built by `+=` — see `row()` for why a single chain won't compile.
            let s = "[parity] straight run done:";
            s += " v_term ray=" + r2(this.vterm_rc);
            s += " phy=" + r2(this.vterm_ph);
            s += " | v_peak ray=" + r2(this.vmax_rc);
            s += " phy=" + r2(this.vmax_ph);
            s += " | dist ray=" + r2(this.d_rc);
            s += " phy=" + r2(this.d_ph);
            print(s);
            print("[parity] STEER phase: throttle 1.0, steer +0.6, both rovers");
        }
        return;
    }

    // ── Phase 3: full throttle + steady steer ───────────────────────────────
    if this.phase == "steer" {
        drive(this.rc, 1.0, 0.6);
        drive(this.ph, 1.0, 0.6);

        // ACCUMULATE the turn tick by tick. Comparing the start forward vector
        // to the current one cannot represent more than half a revolution:
        // `yaw_delta_deg` is an `acos`, so it saturates at 180° and then reads
        // as a turn the OTHER way. A skid rover at full steer passes 180° well
        // inside this phase — the first run logged raycast going 170.9° then
        // −164.8° and the test called it "steered OPPOSITE ways" when both were
        // turning the same direction. Per-tick deltas are always far below the
        // wrap, so the running sum is the true swept angle, unbounded.
        let f_rc = world_forward(this.rc);
        let f_ph = world_forward(this.ph);
        let dy_rc = yaw_delta_deg(this.fprev_rc, f_rc);
        let dy_ph = yaw_delta_deg(this.fprev_ph, f_ph);
        if type_of(dy_rc) != "()" { this.yaw_rc += dy_rc; }
        if type_of(dy_ph) != "()" { this.yaw_ph += dy_ph; }
        this.fprev_rc = f_rc;
        this.fprev_ph = f_ph;

        if t >= this.next_sample {
            let y_rc = this.yaw_rc;
            let y_ph = this.yaw_ph;
            let s = "[parity] steer t=" + r2(t) + "s";
            s += "  v_ray=" + r2(v_rc);
            s += "  v_phy=" + r2(v_ph);
            s += "  yaw_ray=" + y_rc + "deg";
            s += "  yaw_phy=" + y_ph + "deg";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_steer_end {
            // `yaw_rc`/`yaw_ph` are already the accumulated swept angles — do
            // NOT recompute them from start-vs-end here (that reintroduces the
            // 180° wrap this phase exists to avoid).
            brake(this.rc);
            brake(this.ph);
            this.phase = "done";
            // An explicit map, NOT `this`: helpers never see `this` (the runtime
            // binds it only in the hook it calls directly), and passing a plain
            // map keeps the verdict function a pure function of measurements.
            verdict(#{
                vterm_rc: this.vterm_rc, vterm_ph: this.vterm_ph,
                vmax_rc:  this.vmax_rc,  vmax_ph:  this.vmax_ph,
                d_rc:     this.d_rc,     d_ph:     this.d_ph,
                yaw_rc:   this.yaw_rc,   yaw_ph:   this.yaw_ph,
                v_ceiling: this.v_ceiling,
                vmin_rc: this.vmin_rc, vmax_plateau_rc: this.vmax_plateau_rc,
                vmin_ph: this.vmin_ph, vmax_plateau_ph: this.vmax_plateau_ph,
                ymin_rc: this.ymin_rc, ymax_rc: this.ymax_rc,
                ymin_ph: this.ymin_ph, ymax_ph: this.ymax_ph,
                mp:       this.mp
            });
        }
        return;
    }
}

/// Stop the rovers if the scenario is detached or hot-reloaded mid-run.
fn on_stop(me) {
    let rc = find("/DrivetrainParity/RoverRaycast");
    let ph = find("/DrivetrainParity/RoverPhysical");
    if rc >= 0 { brake(rc); }
    if ph >= 0 { brake(ph); }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// TOLERANCES — why these numbers.
//
// The two drivetrains are genuinely different SOLVERS, not two spellings of one
// integrator: the raycast wheel applies a rolled-off force to the body at the
// contact point, while the joint wheel spins a motorised revolute and transmits
// through tire friction with real slip. Demanding bit-parity would be a test of
// the solvers, not of the shared parameter set. Demanding nothing would let the
// 5x bug back in. So each tolerance is set well below the defect it guards and
// well above the honest solver difference:
//
//   terminal speed  +/-15%  Both are pinned to the SAME omega_max * r ceiling,
//                           so at plateau they should agree closely; the residual
//                           is steady-state slip (a few % on the joint side, ~0
//                           on the raycast side). 15% clears that comfortably and
//                           is 30x tighter than the 5x regression it guards.
//   distance        +/-20%  Distance integrates the ACCELERATION transient, where
//                           the two differ most: motor torque through a joint and
//                           tire slip ramps in over ~1-2 s, a direct body force
//                           does not. Looser than speed on purpose, and still far
//                           tighter than any real divergence.
//   yaw response              Both realizations use the same steering geometry
//                           and tire law, so they must turn in the same direction
//                           and agree in swept magnitude within a 20% response
//                           budget. The physical wheel realization resolves
//                           contact loads through its separate bodies, while the
//                           raycast realization applies the same authored tire
//                           law at the reduced chassis; that contact aggregation
//                           and integration order is the bounded difference, not
//                           a second steering coefficient.
//
// Each number above IS the constant passed to `t_rel` below. They are a physical
// argument about two integrators, not a dial: a tolerance changed without the
// reasoning changing with it turns this file into a record of what the engine
// happened to do, and the yaw check — the one that guards the 5x defect — is the
// first thing a uniform tightening silences.
//
// The ABSOLUTE ceiling is the real anchor. Parity alone is satisfiable by both
// rovers being wrong together, so both must also land in
// [0.2, 1.25] * omega_max * r = [0.96, 6.0] m/s. The historic bug put the raycast
// rover at ~24 m/s; the floor catches a dead command surface.
fn verdict(s) {
    let f = [];

    // ── The vehicle itself, before any behaviour ────────────────────────────
    // A rover that masses differently, or acts its mass at a different height,
    // cannot be made to match by tuning a tire — and CoM HEIGHT IS LOAD
    // TRANSFER, so it lands squarely on the turning comparison below. These
    // read the components the SOLVER integrates, so an override avian silently
    // discarded shows up here as an unchanged number rather than as a mystery.
    if s.mp == () {
        f.push("mass properties: NEVER SAMPLED (no `Computed*` on the rovers?)");
    } else {
        let mp = s.mp;
        // Anti-trivial: four wheel bodies, or the physical aggregate is a fiction.
        f.push(t_true(mp.wheels == 4,
                      "physical rover exposed " + mp.wheels + " wheel bodies, expected 4"));
        f.push(t_rel(mp.m_rc, mp.m_ph, 1.0, "vehicle mass (kg)"));

        let m_com = "centre of mass height: raycast " + r2(mp.com_rc);
        m_com += " m vs physical aggregate " + r2(mp.com_ph);
        m_com += " m - the same mass acting at two different heights";
        let dcom = mp.com_rc - mp.com_ph;
        f.push(t_true(dcom.abs() < 0.01, m_com));

        let mrow = "[parity]   mass (kg)         " + r2(mp.m_rc) + "       " + r2(mp.m_ph);
        print(mrow);
        let crow = "[parity]   com_y (m)         " + r2(mp.com_rc) + "       " + r2(mp.com_ph);
        print(crow);
        let irow = "[parity]   I_yy chassis      " + r2(mp.iyy_rc) + "       " + r2(mp.iyy_ch);
        print(irow + "   (raycast folded; physical chassis-only)");
    }

    f.push(t_rel(s.vterm_rc, s.vterm_ph, 15.0, "terminal speed (m/s)"));
    f.push(t_rel(s.vmax_rc, s.vmax_ph, 15.0, "peak speed (m/s)"));
    f.push(t_rel(s.d_rc, s.d_ph, 20.0, "distance travelled (m)"));

    // Absolute ceiling + floor: both must honour omega_max * r, not merely match.
    // LUNAR TRACTION sets the floor, not the drivetrain. `omega_max * r` is a
    // KINEMATIC ceiling — a driven wheel cannot out-run its own no-slip speed —
    // and it holds at any gravity, so the upper bound stays tied to it. The
    // lower bound cannot: at 1.62 m/s^2 the normal load is a sixth of Earth's,
    // the rover runs out of grip long before it runs out of gearing, and the
    // measured terminal speed is 2.2-2.4 m/s against a 4.8 m/s ceiling. Asking
    // it to reach half the ceiling would be asserting Earth traction on the
    // Moon. The floor is therefore a LIVENESS gate — proof the command surface
    // actually drove the rover — set far below the measured value and far above
    // a dead test.
    let lo = s.v_ceiling * 0.2;
    let hi = s.v_ceiling * 1.25;
    f.push(t_range(s.vterm_rc, lo, hi,
                   "raycast terminal speed vs authored omega_max*r=" + s.v_ceiling));
    f.push(t_range(s.vterm_ph, lo, hi,
                   "physical terminal speed vs authored omega_max*r=" + s.v_ceiling));

    // High-speed stability: evaluate the whole trailing plateau rather than a
    // single sample.  A forward/back limit cycle leaves the mean speed plausible
    // while widening this span; vertical chassis bob is its usual companion.
    // The limits are deliberately generous compared with steady solver noise but
    // far below the old visible oscillation.  They guard each realization on its
    // own, so two equally unstable rovers cannot pass by matching one another.
    let vspan_rc = s.vmax_plateau_rc - s.vmin_rc;
    let vspan_ph = s.vmax_plateau_ph - s.vmin_ph;
    let yspan_rc = s.ymax_rc - s.ymin_rc;
    let yspan_ph = s.ymax_ph - s.ymin_ph;
    f.push(t_range(vspan_rc, 0.0, 0.75, "raycast high-speed speed ripple (m/s)"));
    f.push(t_range(vspan_ph, 0.0, 0.75, "physical high-speed speed ripple (m/s)"));
    f.push(t_range(yspan_rc, 0.0, 0.08, "raycast high-speed chassis bob (m)"));
    f.push(t_range(yspan_ph, 0.0, 0.08, "physical high-speed chassis bob (m)"));

    // Yaw: each realization must respond to the same command in the same
    // direction, with clear authority, and with the shared tire-law response
    // magnitude. The relative bound covers only contact aggregation and fixed
    // versus substep integration residuals; it is not a fitted yaw multiplier.
    if s.yaw_rc == () || s.yaw_ph == () {
        f.push("heading change: MISSING (world_forward returned () for a rover)");
    } else {
        // Messages hoisted into locals: a concatenation chain passed AS a call
        // argument counts toward the same expression-complexity cap that
        // `row()` documents.
        let m_sign = "heading change SIGN differs: raycast=" + r2(s.yaw_rc);
        m_sign += "deg physical=" + r2(s.yaw_ph);
        m_sign += "deg - the rovers steered OPPOSITE ways";
        f.push(t_true((s.yaw_rc > 0.0) == (s.yaw_ph > 0.0), m_sign));

        let m_small = "heading change: a rover barely turned (raycast=" + r2(s.yaw_rc);
        m_small += "deg physical=" + r2(s.yaw_ph);
        m_small += "deg, expected > 5deg under steer 0.6)";
        f.push(t_true(s.yaw_rc.abs() > 5.0 && s.yaw_ph.abs() > 5.0, m_small));

        let m_yaw = "heading change magnitude (deg): raycast=" + r2(s.yaw_rc.abs());
        m_yaw += " physical=" + r2(s.yaw_ph.abs());
        f.push(t_rel(s.yaw_rc.abs(), s.yaw_ph.abs(), 20.0, m_yaw));
    }

    print("[parity] ── results ────────────────────────────────────────────");
    print("[parity]   quantity          raycast     physical");
    print("[parity]   v_terminal (m/s)  " + r2(s.vterm_rc) + "       " + r2(s.vterm_ph));
    print("[parity]   v_peak     (m/s)  " + r2(s.vmax_rc) + "       " + r2(s.vmax_ph));
    print("[parity]   distance   (m)    " + r2(s.d_rc) + "       " + r2(s.d_ph));
    print("[parity]   speed ripple(m/s) " + r2(vspan_rc) + "       " + r2(vspan_ph));
    print("[parity]   chassis bob (m)  " + r2(yspan_rc) + "       " + r2(yspan_ph));
    print("[parity]   yaw        (deg)  " + s.yaw_rc + "       " + s.yaw_ph);
    print("[parity]   authored ceiling omega_max*r = " + s.v_ceiling + " m/s");

    report_verdict(f, "DRIVETRAIN PARITY", "DRIVETRAIN_PARITY");
}
