// ── Ackermann drivetrain parity test — raycast wheels vs joint wheels ───────
//
// TEST-ONLY. Runs against `scenes/sandbox/ackermann_parity.usda`, which puts the
// SAME rover asset on the SAME ground twice, differing in exactly one authored
// opinion: `variants = { string drivetrain = "raycast" | "physical" }`. This
// script drives BOTH from ONE tick loop — same command, same frame — and asserts
// they behave the same.
//
// The sibling of `drivetrain_parity.rhai`, which does this for the SKID rover.
// Structure, helpers, tolerances and verdict contract are deliberately copied
// from that file: two gates that measure the same physics should not disagree
// about how they measure it, and a reader who has understood one has understood
// both. What is NEW here is everything to do with STEERING.
//
// ── WHY A SECOND PARITY GATE ────────────────────────────────────────────────
//
// A skid rover has no steering actuator at all. It turns by driving one side
// harder than the other, so the skid gate exercises exactly one path: drive
// allocation -> wheel drive force. An Ackermann rover turns by POINTING its
// front wheels, which brings in three more shared mechanisms that the skid gate
// cannot reach and that can each diverge between the two wheel realizations:
//
//   * the `linear` allocation kernel and its dedicated `steering` port
//     (`drive_left=1,0 drive_right=1,0 steering=0,1`) rather than a skid mix;
//   * `lunco_hardware::SteeringActuator` — the shared, rate-limited steer servo
//     both wheel kinds slew, one applying its output to a joint frame basis and
//     the other to a wheel transform;
//   * `physxVehicleAckermannSteering:maxSteerAngle`, a VEHICLE-level lock that
//     both realizations must read from the same place.
//
// Nothing else in the repository executes those against both drivetrains.
//
// ── WHAT THE ZERO-THROTTLE PIVOT PHASE IS FOR ───────────────────────────────
//
// The sharpest assertion in this file is a NEGATIVE one. Command full steer with
// ZERO throttle and an Ackermann rover must NOT rotate: the drive ports carry no
// steering term (`drive_left = throttle*1 + steer*0`), so at zero throttle there
// is no wheel force at all and pointing the front wheels moves nothing. A SKID
// rover under that same command pivots briskly on the spot.
//
// That makes the pivot phase a direct, falsifiable test of the ALLOCATION SHAPE
// and not merely of the vehicle: if Ackermann allocation ever silently collapses
// back to a differential mix — the exact failure mode a "move allocation into
// authored Modelica programs" refactor can introduce, since a shared drive law
// that fans throttle+steer onto left/right is the obvious wrong default — this
// rover starts spinning under zero throttle and the check fires. A test that
// only ever asks "did it turn?" would call that regression a pass.
//
// ── THE ABSOLUTE CEILING ────────────────────────────────────────────────────
//
// Both realizations read ONE authored no-load axle speed: the motor's
// `lunco:motor:noLoadSpeed` (14400 rad/s, `components/mobility/motor.usda`)
// divided by the gearbox `lunco:gearbox:ratio` (1200,
// `components/mobility/gearbox.usda`) = 12 rad/s at the axle. The joint motor
// targets it; the raycast drive force rolls off toward it. With r = 0.4 m both
// must cap at
//
//     v_max = omega_max * r = 12.0 * 0.4 = 4.8 m/s
//
// Parity alone is satisfiable by both rovers being wrong together, so both must
// ALSO land inside a window around that number.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// There is NO exit-code mechanism for a scenario — a scenario is a tick hook on
// a live sim, not a process. So this file uses the verdict contract the rhai
// test harness established: checks return "" on pass or a message on failure,
// and the run ends with a greppable verdict on stdout (the process log):
//
//     TESTS_OK <n>                 or   TESTS_FAIL <n>/<m>   (+ "  FAIL ..." lines)
//     ACKERMANN PARITY: PASS       or   ACKERMANN PARITY: FAIL      <- LAST line
//
// A caller greps for `ACKERMANN PARITY:`. The suite ALSO emits `ACKERMANN_PARITY`
// on the telemetry bus with value "PASS"/"FAIL" and raises a toast, so a UI run
// is not silent either.
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
//     evidence, and a run with no `[ackermann]` sample lines proves nothing.
//   * a rover that never reaches half the expected top speed fails as "did not
//     move", so a dead command surface cannot masquerade as parity (0 == 0).
//   * the pivot check is the one place where NOT moving is the pass, so it is
//     paired with the drive and steer phases that require large motion — a dead
//     rover fails those first and can never reach a free pass on the pivot.
// If you do not see a sample table and a verdict line, the test did not run.

// ── Local assertion helpers (stateless — `this` is NOT bound in helpers) ────

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT (matching the steer-sign convention in `prelude/nav.rhai`, where +steer
/// yaws right). Sign is taken from the yaw-plane cross product, so a rover that
/// turned the WRONG way cannot pass by magnitude alone.
/// Find one `DriveMix.entries` term by port name, or `()`.
///
/// `get(rover, "DriveMix.entries")` reflects the live allocation table out of the
/// ECS as an array of maps (`#{ port, forward, steer, brake }`) — the AUTHORED
/// mix as the engine actually built it, not as the USD says it should be. That
/// distinction is the point: the entries check below fails if the reader stops
/// producing the table even while the USD still describes it.
fn mix_entry(entries, port) {
    if type_of(entries) != "array" { return (); }
    for e in entries {
        if type_of(e) != "map" { continue; }
        if e.port == port { return e; }
    }
    ()
}

/// One log row. The table is the evidence; keep it readable.
///
/// Built by successive `+=` rather than one concatenation chain: rhai caps
/// EXPRESSION complexity, and a single six-value chain trips
/// "Expression exceeds maximum complexity" at compile time.
fn row(tag, t, v_rc, v_ph, d_rc, d_ph) {
    let s = "[ackermann] " + tag;
    s += " t=" + r2(t) + "s";
    s += "  v_ray=" + r2(v_rc);
    s += "  v_phy=" + r2(v_ph);
    s += "  d_ray=" + r2(d_rc);
    s += "  d_phy=" + r2(d_ph);
    print(s);
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    // Tunables live on `this`: named fns cannot see top-level `const`s.
    this.t = 0.0;
    this.settle_s = 3.0;    // let both rovers land and stop bouncing
    this.drive_s = 12.0;    // full throttle, straight — long enough to plateau
    this.steer_s = 6.0;     // full throttle + steady steer — yaw authority
    this.pivot_s = 5.0;     // ZERO throttle + full steer — must NOT rotate
    this.plateau_s = 3.0;   // trailing window of the drive phase averaged as v_terminal
    this.sample_s = 0.5;    // log cadence

    // The authored contract under test: motor no-load speed / gearbox ratio.
    this.omega_max = 12.0;
    this.wheel_r = 0.4;
    this.v_ceiling = 4.8;   // omega_max * wheel_r

    // Steer command used in the steer phase. 0.8 rather than 1.0 leaves the
    // servo off its hard lock, so the two realizations are compared while the
    // shared `SteeringActuator` is still in its linear range instead of both
    // being pinned by the same clamp (which would make almost any steering bug
    // look like parity).
    this.steer_cmd = 0.8;

    this.phase = "settle";
    this.next_sample = 0.0;

    this.rc = find("/AckermannParity/RoverRaycast");
    this.ph = find("/AckermannParity/RoverPhysical");

    this.d_rc = 0.0;   this.d_ph = 0.0;
    this.p_rc = ();    this.p_ph = ();
    this.vmax_rc = 0.0; this.vmax_ph = 0.0;
    this.vsum_rc = 0.0; this.vsum_ph = 0.0; this.vn = 0;
    this.vterm_rc = 0.0; this.vterm_ph = 0.0;

    // Accumulated swept yaw (degrees), per phase, plus the previous tick's
    // forward vector each accumulation differences against. Start at 0.0, not
    // `()`: these are SUMMED into.
    this.ystr_rc = 0.0; this.ystr_ph = 0.0;   // straight phase — expected ~0
    this.yaw_rc = 0.0;  this.yaw_ph = 0.0;    // steer phase — expected large
    this.ypiv_rc = 0.0; this.ypiv_ph = 0.0;   // pivot phase — expected ~0
    this.dpiv_rc = 0.0; this.dpiv_ph = 0.0;   // pivot travel — expected ~0
    this.fprev_rc = (); this.fprev_ph = ();

    // Pivot-phase clock. Declared here rather than created on the fly when the
    // phase starts: every field this hook reads should exist from tick one, so a
    // typo is a wrong VALUE rather than a silently absent key.
    this.pivot_start = 0.0;
    this.pivot_settle_s = 2.0;   // brake to rest before measuring "did not move"

    // The steer servo's realised deflection on the raycast rover's front wheel,
    // sampled at the end of the pivot phase. See `verdict` for why this is read
    // on ONE side only.
    this.deflect_rc = ();

    print("[ackermann] ── ackermann drivetrain parity test ──────────────────");
    print("[ackermann] raycast rover gid=" + this.rc + "  physical rover gid=" + this.ph);
    print("[ackermann] contract: omega_max=" + this.omega_max + " rad/s * r=" + this.wheel_r +
          " m = " + this.v_ceiling + " m/s for BOTH realizations");
    print("[ackermann] phases: settle " + this.settle_s + "s -> straight " + this.drive_s +
          "s -> steer " + this.steer_s + "s -> zero-throttle pivot " + this.pivot_s + "s");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op: `find` yields -1 and every
    // subsequent read is (). Fail on the FIRST tick instead of ticking forever.
    if this.rc == () || this.ph == () || this.rc < 0 || this.ph < 0 {
        print("  PARITY FAIL: rover prims not found (raycast gid=" + this.rc +
              ", physical gid=" + this.ph + ") - wrong scene? expected " +
              "scenes/sandbox/ackermann_parity.usda");
        print("TESTS_FAIL 1/1");
        print("ACKERMANN PARITY: FAIL");
        emit("ACKERMANN_PARITY", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let t = this.t;
    let t_drive_end = this.settle_s + this.drive_s;
    let t_steer_end = t_drive_end + this.steer_s;
    let t_pivot_end = t_steer_end + this.pivot_s;

    let pos_rc = world_pos(this.rc);
    let pos_ph = world_pos(this.ph);

    // Speed from POSITION DELTA, not `speed()`. `speed()` reports 0.0 for these
    // rovers even while they visibly travel tens of metres — it reads a velocity
    // source these vessels do not publish, and trusting it produces a "both
    // ~zero, so they match" reading, which is exactly the dead-test trap this
    // file refuses. Displacement per tick is measured from the same `world_pos`
    // the odometer uses, so speed and distance can never disagree about whether
    // the rover moved.
    let d_t = dt();
    let v_rc = if d_t > 0.0 { seg(this.p_rc, pos_rc) / d_t } else { 0.0 };
    let v_ph = if d_t > 0.0 { seg(this.p_ph, pos_ph) / d_t } else { 0.0 };

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
            this.fprev_rc = world_forward(this.rc);
            this.fprev_ph = world_forward(this.ph);
            this.next_sample = t;
            let s = "[ackermann] settled (v_ray=" + r2(v_rc);
            s += " v_phy=" + r2(v_ph);
            s += ") - FULL THROTTLE, steer 0, both rovers, same tick";
            print(s);
        }
        return;
    }

    // ── Phase 2: full throttle, steer 0 ─────────────────────────────────────
    if this.phase == "drive" {
        drive(this.rc, 1.0, 0.0);
        drive(this.ph, 1.0, 0.0);

        this.d_rc += seg(this.p_rc, pos_rc);
        this.d_ph += seg(this.p_ph, pos_ph);
        this.p_rc = pos_rc;
        this.p_ph = pos_ph;
        if v_rc > this.vmax_rc { this.vmax_rc = v_rc; }
        if v_ph > this.vmax_ph { this.vmax_ph = v_ph; }

        // Straight-line yaw drift. Accumulated, not start-vs-end, for the same
        // wrap reason the steer phase documents — and it is a REAL check, not
        // bookkeeping: an Ackermann rover commanded steer 0 must track straight,
        // and a drive allocation that leaked a steering term into the drive
        // ports would show up here as a rover that curves under pure throttle.
        let fs_rc = world_forward(this.rc);
        let fs_ph = world_forward(this.ph);
        let ds_rc = yaw_delta_deg(this.fprev_rc, fs_rc);
        let ds_ph = yaw_delta_deg(this.fprev_ph, fs_ph);
        if type_of(ds_rc) != "()" { this.ystr_rc += ds_rc; }
        if type_of(ds_ph) != "()" { this.ystr_ph += ds_ph; }
        this.fprev_rc = fs_rc;
        this.fprev_ph = fs_ph;

        // Terminal speed = mean over the trailing plateau window, not a single
        // frame: one noisy tick is not a top speed, and the two solvers ripple
        // differently around the same mean.
        if t >= t_drive_end - this.plateau_s {
            this.vsum_rc += v_rc;
            this.vsum_ph += v_ph;
            this.vn += 1;
        }

        if t >= this.next_sample {
            row("drive", t, v_rc, v_ph, this.d_rc, this.d_ph);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_drive_end {
            // Plain statements, not inline `if` EXPRESSIONS: rhai caps expression
            // complexity and `if cond { a.b / c.to_float() } else { 0.0 }` as a
            // right-hand side trips it (see `row()` for the same limit on
            // concatenation chains).
            this.vterm_rc = 0.0;
            this.vterm_ph = 0.0;
            if this.vn > 0 {
                let n = this.vn.to_float();
                this.vterm_rc = this.vsum_rc / n;
                this.vterm_ph = this.vsum_ph / n;
            }
            this.phase = "steer";
            // Built by `+=` — see `row()` for why a single chain won't compile.
            let s = "[ackermann] straight run done:";
            s += " v_term ray=" + r2(this.vterm_rc);
            s += " phy=" + r2(this.vterm_ph);
            s += " | v_peak ray=" + r2(this.vmax_rc);
            s += " phy=" + r2(this.vmax_ph);
            s += " | dist ray=" + r2(this.d_rc);
            s += " phy=" + r2(this.d_ph);
            print(s);
            let s2 = "[ackermann] straight-line yaw drift (expect ~0):";
            s2 += " ray=" + r2(this.ystr_rc) + "deg";
            s2 += " phy=" + r2(this.ystr_ph) + "deg";
            print(s2);
            print("[ackermann] STEER phase: throttle 1.0, steer +" + this.steer_cmd);
        }
        return;
    }

    // ── Phase 3: full throttle + steady steer ───────────────────────────────
    if this.phase == "steer" {
        drive(this.rc, 1.0, this.steer_cmd);
        drive(this.ph, 1.0, this.steer_cmd);

        // ACCUMULATE the turn tick by tick. Comparing the start forward vector
        // to the current one cannot represent more than half a revolution:
        // `yaw_delta_deg` is an `acos`, so it saturates at 180 degrees and then
        // reads as a turn the OTHER way. Per-tick deltas are always far below
        // the wrap, so the running sum is the true swept angle, unbounded.
        let f_rc = world_forward(this.rc);
        let f_ph = world_forward(this.ph);
        let dy_rc = yaw_delta_deg(this.fprev_rc, f_rc);
        let dy_ph = yaw_delta_deg(this.fprev_ph, f_ph);
        if type_of(dy_rc) != "()" { this.yaw_rc += dy_rc; }
        if type_of(dy_ph) != "()" { this.yaw_ph += dy_ph; }
        this.fprev_rc = f_rc;
        this.fprev_ph = f_ph;

        this.p_rc = pos_rc;
        this.p_ph = pos_ph;

        if t >= this.next_sample {
            let y_rc = this.yaw_rc;
            let y_ph = this.yaw_ph;
            let s = "[ackermann] steer t=" + r2(t) + "s";
            s += "  v_ray=" + r2(v_rc);
            s += "  v_phy=" + r2(v_ph);
            s += "  yaw_ray=" + r2(y_rc) + "deg";
            s += "  yaw_phy=" + r2(y_ph) + "deg";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_steer_end {
            // Come to a stop before the pivot phase: the pivot assertion is that
            // the rover does not TRAVEL and does not TURN under steer-only, and
            // carried-over momentum from a 4.8 m/s run would answer that question
            // with inertia instead of with allocation. The brake window is part
            // of the pivot phase's own clock, handled by `pivot_settle_s` below.
            brake(this.rc);
            brake(this.ph);
            this.phase = "pivot";
            this.pivot_start = t;
            let s = "[ackermann] steer run done: yaw ray=" + r2(this.yaw_rc);
            s += "deg phy=" + r2(this.yaw_ph) + "deg";
            print(s);
            print("[ackermann] PIVOT phase: braking to rest, then throttle 0.0 + steer 1.0");
            print("[ackermann]   an Ackermann rover MUST NOT rotate here - no throttle, no drive force,");
            print("[ackermann]   so pointed front wheels move nothing. A skid mix would spin on the spot.");
        }
        return;
    }

    // ── Phase 4: ZERO throttle + full steer — the allocation-shape check ────
    if this.phase == "pivot" {
        let since = t - this.pivot_start;
        if since < this.pivot_settle_s {
            // Still shedding the steer phase's momentum. Brake, and measure
            // nothing: motion here belongs to the previous phase.
            brake(this.rc);
            brake(this.ph);
            this.p_rc = pos_rc;
            this.p_ph = pos_ph;
            this.fprev_rc = world_forward(this.rc);
            this.fprev_ph = world_forward(this.ph);
            return;
        }

        // Throttle EXACTLY zero, steer at the lock. With the authored Ackermann
        // mix (`drive_left=1,0 drive_right=1,0 steering=0,1`) both drive ports
        // read `1.0*0 + steer*0 = 0` and only the steering port is live.
        drive(this.rc, 0.0, 1.0);
        drive(this.ph, 0.0, 1.0);

        this.dpiv_rc += seg(this.p_rc, pos_rc);
        this.dpiv_ph += seg(this.p_ph, pos_ph);
        this.p_rc = pos_rc;
        this.p_ph = pos_ph;

        let f_rc = world_forward(this.rc);
        let f_ph = world_forward(this.ph);
        let dp_rc = yaw_delta_deg(this.fprev_rc, f_rc);
        let dp_ph = yaw_delta_deg(this.fprev_ph, f_ph);
        if type_of(dp_rc) != "()" { this.ypiv_rc += dp_rc; }
        if type_of(dp_ph) != "()" { this.ypiv_ph += dp_ph; }
        this.fprev_rc = f_rc;
        this.fprev_ph = f_ph;

        if t >= this.next_sample {
            let s = "[ackermann] pivot t=" + r2(t) + "s";
            s += "  yaw_ray=" + r2(this.ypiv_rc) + "deg";
            s += "  yaw_phy=" + r2(this.ypiv_ph) + "deg";
            s += "  moved_ray=" + r2(this.dpiv_rc) + "m";
            s += "  moved_phy=" + r2(this.dpiv_ph) + "m";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if t >= t_pivot_end {
            // Sample the steer servo's REALISED deflection while the command is
            // still at the lock — read now, not after braking, or the servo has
            // already slewed back toward centre.
            let fl = find("/AckermannParity/RoverRaycast/Wheel_FL");
            if type_of(fl) != "()" && fl >= 0 {
                this.deflect_rc = get(fl, "SteeringActuator.output_angle");
            }

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
                ystr_rc:  this.ystr_rc,  ystr_ph:  this.ystr_ph,
                yaw_rc:   this.yaw_rc,   yaw_ph:   this.yaw_ph,
                ypiv_rc:  this.ypiv_rc,  ypiv_ph:  this.ypiv_ph,
                dpiv_rc:  this.dpiv_rc,  dpiv_ph:  this.dpiv_ph,
                deflect_rc: this.deflect_rc,
                kernel_rc: get(this.rc, "DriveMix.kernel"),
                kernel_ph: get(this.ph, "DriveMix.kernel"),
                entries_rc: get(this.rc, "DriveMix.entries"),
                v_ceiling: this.v_ceiling,
                steer_cmd: this.steer_cmd
            });
        }
        return;
    }
}

/// Stop the rovers if the scenario is detached or hot-reloaded mid-run.
fn on_stop(me) {
    let rc = find("/AckermannParity/RoverRaycast");
    let ph = find("/AckermannParity/RoverPhysical");
    if rc >= 0 { brake(rc); }
    if ph >= 0 { brake(ph); }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// TOLERANCES — why these numbers. They are the ones `drivetrain_parity.rhai`
// arrived at, for the reasons that file records: the two drivetrains are
// genuinely different SOLVERS (a rolled-off body force at the contact point vs a
// motorised revolute transmitting through tire slip), so demanding bit-parity
// would test the solvers rather than the shared parameter set, and demanding
// nothing would let a 5x divergence back in.
//
//   terminal speed  +/-15%  Both pinned to the SAME omega_max * r ceiling.
//   distance        +/-20%  Integrates the acceleration transient, where the two
//                           differ most. Looser on purpose.
//   yaw change      +/-35%  The most solver-sensitive quantity. This check exists
//                           to catch a rover that barely turns or turns the WRONG
//                           way, not to pin a yaw rate. SIGN is checked
//                           separately and is NOT tolerant.
//
// ACKERMANN-SPECIFIC BOUNDS, and why they are absolute rather than relative:
//
//   straight drift  <= 25 deg over a 12 s full-throttle run. Not a tight
//                   tracking spec — a raycast rover and a joint rover wander
//                   differently on their suspensions — but far below the ~90 deg
//                   a leaked steering term in the drive ports would produce.
//   pivot yaw       <= 12 deg over the measured pivot window. This is THE
//                   allocation-shape assertion. A skid mix under the same
//                   command sweeps several hundred degrees, so the margin
//                   between pass and the failure it guards is enormous; the
//                   allowance exists only for the residual yaw of a rover
//                   settling on its springs with its front wheels turned.
//   pivot travel    <= 1.5 m, for the same reason and with the same margin: at
//                   zero throttle there is no drive force, so any real travel
//                   means the drive ports are being fed a steering term.
//   steer deflect   >= 0.05 rad. Proves the `steering` port reaches an actuator
//                   at all, rather than the rover merely happening to turn.
//
// WHY `deflect` IS READ ON THE RAYCAST ROVER ONLY. `SteeringActuator` is the
// SHARED steer model, but the two realizations mount it in different places: the
// raycast wheel carries it on the wheel entity (`apply_wheel_steering` queries
// `(&mut Transform, &SteeringActuator, &WheelRaycast)`), while the joint wheel
// carries it on the JOINT entity, which is not a USD prim and so has no gid a
// scenario can `find`. Reading it on both sides would report a permanent MISSING
// for the physical rover and turn a real check into noise. The physical rover's
// steering is covered by its MOTION instead — it must sweep the same yaw as the
// raycast rover under the same command, which it cannot do without a working
// steering actuator.
fn verdict(s) {
    let f = [];

    // ── Straight-line parity ────────────────────────────────────────────────
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

    // Steer 0 must mean steer 0: the drive ports carry no steering term.
    f.push(t_max(s.ystr_rc, 25.0, "raycast yaw drift under steer 0 (deg)"));
    f.push(t_max(s.ystr_ph, 25.0, "physical yaw drift under steer 0 (deg)"));

    // ── Steer parity: direction first (intolerant), then magnitude (loose) ──
    if s.yaw_rc == () || s.yaw_ph == () {
        f.push("heading change: MISSING (world_forward returned () for a rover)");
    } else {
        // Messages hoisted into locals: a concatenation chain passed AS a call
        // argument counts toward the same expression-complexity cap that `row()`
        // documents.
        let m_sign = "heading change SIGN differs: raycast=" + r2(s.yaw_rc);
        m_sign += "deg physical=" + r2(s.yaw_ph);
        m_sign += "deg - the rovers steered OPPOSITE ways";
        f.push(t_true((s.yaw_rc > 0.0) == (s.yaw_ph > 0.0), m_sign));

        let m_small = "heading change: a rover barely turned (raycast=" + r2(s.yaw_rc);
        m_small += "deg physical=" + r2(s.yaw_ph);
        m_small += "deg, expected > 15deg under steer " + s.steer_cmd + ")";
        f.push(t_true(s.yaw_rc.abs() > 15.0 && s.yaw_ph.abs() > 15.0, m_small));

        f.push(t_rel(s.yaw_rc, s.yaw_ph, 35.0, "heading change (deg)"));

        // Steer authority is REAL, not drift: the commanded turn must dominate
        // the same rover's own unsteered wander by a wide margin. Comparing a
        // rover against ITSELF makes this immune to any absolute yaw-rate
        // tuning — only the ratio matters.
        let m_auth = "steer authority: raycast turned " + r2(s.yaw_rc.abs());
        m_auth += "deg under steer but drifted " + r2(s.ystr_rc.abs());
        m_auth += "deg under steer 0 - steering is not distinguishable from drift";
        f.push(t_true(s.yaw_rc.abs() > s.ystr_rc.abs() * 2.0 + 10.0, m_auth));
    }

    // ── The allocation-shape check: no pivot without throttle ───────────────
    let m_piv_rc = "raycast ROTATED under zero throttle (" + r2(s.ypiv_rc);
    m_piv_rc += "deg): an Ackermann rover steers with a dedicated `steering` port, so";
    m_piv_rc += " zero throttle means zero drive force and zero yaw. This is the";
    m_piv_rc += " signature of allocation collapsing back to a differential/skid mix.";
    f.push(t_true(s.ypiv_rc.abs() <= 12.0, m_piv_rc));

    let m_piv_ph = "physical ROTATED under zero throttle (" + r2(s.ypiv_ph);
    m_piv_ph += "deg): see the raycast message - the joint drivetrain is being fed";
    m_piv_ph += " a steering term on its DRIVE ports.";
    f.push(t_true(s.ypiv_ph.abs() <= 12.0, m_piv_ph));

    f.push(t_max(s.dpiv_rc, 1.5, "raycast travel under zero throttle (m)"));
    f.push(t_max(s.dpiv_ph, 1.5, "physical travel under zero throttle (m)"));

    // ── The steering port exists, is allocated to, and reaches an actuator ──
    f.push(t_true(s.kernel_rc == "linear",
                  "raycast drive kernel is '" + s.kernel_rc + "', expected 'linear'"));
    f.push(t_true(s.kernel_ph == "linear",
                  "physical drive kernel is '" + s.kernel_ph + "', expected 'linear'"));

    let steering = mix_entry(s.entries_rc, "steering");
    if type_of(steering) == "()" {
        f.push("allocation has NO `steering` entry - an Ackermann rover with no steering port");
    } else {
        let m_st = "steering entry is forward=" + steering.forward;
        m_st += " steer=" + steering.steer;
        m_st += ", expected forward=0 steer=1 (steering is driven by STEER alone)";
        f.push(t_true(steering.forward.abs() < 1e-6 && steering.steer > 0.5, m_st));
    }

    let dl = mix_entry(s.entries_rc, "drive_left");
    if type_of(dl) == "()" {
        f.push("allocation has NO `drive_left` entry - the rover has no drive port");
    } else {
        let m_dl = "drive_left entry is forward=" + dl.forward;
        m_dl += " steer=" + dl.steer;
        m_dl += ", expected steer=0: an Ackermann drive port carries NO steering term";
        f.push(t_true(dl.forward > 0.5 && dl.steer.abs() < 1e-6, m_dl));
    }

    let m_def = "steer servo deflection on RoverRaycast/Wheel_FL: " + s.deflect_rc;
    m_def += " rad, expected >= 0.05 - the `steering` port never reached the actuator";
    if type_of(s.deflect_rc) == "()" {
        f.push("steer servo deflection: MISSING - no SteeringActuator on RoverRaycast/Wheel_FL");
    } else {
        f.push(t_true(s.deflect_rc.abs() >= 0.05, m_def));
    }

    print("[ackermann] ── results ─────────────────────────────────────────");
    print("[ackermann]   quantity            raycast     physical");
    print("[ackermann]   v_terminal (m/s)    " + r2(s.vterm_rc) + "       " + r2(s.vterm_ph));
    print("[ackermann]   v_peak     (m/s)    " + r2(s.vmax_rc) + "       " + r2(s.vmax_ph));
    print("[ackermann]   distance   (m)      " + r2(s.d_rc) + "       " + r2(s.d_ph));
    print("[ackermann]   yaw@steer0 (deg)    " + r2(s.ystr_rc) + "       " + r2(s.ystr_ph));
    print("[ackermann]   yaw@steer  (deg)    " + r2(s.yaw_rc) + "       " + r2(s.yaw_ph));
    print("[ackermann]   yaw@pivot  (deg)    " + r2(s.ypiv_rc) + "       " + r2(s.ypiv_ph));
    print("[ackermann]   travel@pivot (m)    " + r2(s.dpiv_rc) + "       " + r2(s.dpiv_ph));
    print("[ackermann]   kernel              " + s.kernel_rc + "       " + s.kernel_ph);
    print("[ackermann]   front-wheel steer (rad, raycast) " + s.deflect_rc);
    print("[ackermann]   authored ceiling omega_max*r = " + s.v_ceiling + " m/s");

    report_verdict(f, "ACKERMANN PARITY", "ACKERMANN_PARITY");
}
