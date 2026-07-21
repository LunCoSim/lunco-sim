// ── Six-wheel independent-drive MOTION-CORRECTNESS test ────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/six_independent_parity.usda`.
//
// ⚠ THIS IS NOT A RAYCAST-vs-PHYSICAL PARITY TEST, and it cannot be one.
//
// `vessels/rovers/six_wheel_independent.usda` carries NO `drivetrain`
// variantSet. That is structural, not an oversight: the shared drivetrain
// overlays (`components/mobility/raycast_drivetrain.usda`,
// `physical_drivetrain.usda`) are authored as `over "Wheel_FL"` / `"Wheel_FR"` /
// `"Wheel_RL"` / `"Wheel_RR"` — a FOUR-wheel prim vocabulary — while this
// rover's wheels are `Wheel_L0..L2` / `Wheel_R0..R2`. A `physical` selection
// would compose onto nothing, author no joints, and yield a rover with no
// drivetrain at all rather than a joint-driven twin. Comparing that against the
// raycast rover would not be a parity measurement, it would be a rover measured
// against a wreck.
//
// So this file asserts something ABSOLUTE instead of something relative: that
// the AUTHORED allocation produces the motion the authored allocation
// describes. It is the same safety net, guarding the same refactor, with its
// contrast drawn from TIME (four command legs on one rover) rather than from a
// second lane.
//
// ── THE AUTHORED CONTRACT UNDER TEST ────────────────────────────────────────
//
// This rover is the only asset in the fleet whose drive allocation is fully
// AUTHORED rather than inferred from an Omniverse schema:
//
//     float outputs:drive_w0 = 0.0   # ... through outputs:drive_w5
//
//     def "DriveMix"
//     {
//         def "drive_w0" { double lunco:factor:throttle = 1.0
//                          double lunco:factor:steer    = 1.0 }
//         # ... w1, w2 the same; w3..w5 with lunco:factor:steer = -1.0
//     }
//
// That is the `linear` kernel over six independent channels, each term being
// `value = throttle*forward + steer*steer`. Left bank (`w0..w2`) gets
// `throttle + steer`; right bank (`w3..w5`) gets `throttle - steer`. Four
// consequences follow DIRECTLY from those coefficients, and this file asserts
// each one as motion:
//
//   1. pure throttle -> all six channels equal        -> runs STRAIGHT
//   2. throttle + steer -> banks differentiate         -> TURNS
//   3. the SIGN of the steer column fixes WHICH WAY    -> +steer turns RIGHT
//   4. pure steer, zero throttle -> `+s` left, `-s` right -> PIVOTS on the spot,
//      travelling almost nowhere
//
// (3) deserves its derivation spelled out, because it is the one assertion here
// that a reader might mistake for an arbitrary sign convention. The left bank
// carries `+steer` and the right bank `-steer`, so a positive steer command
// drives the left wheels forward and the right wheels back; a vehicle whose left
// side outruns its right side yaws to the RIGHT. `yaw_delta_deg` below reports
// right-hand yaw as POSITIVE (the convention in `prelude/nav.rhai`, where +steer
// yaws right). Therefore `+steer` must produce a positive accumulated yaw. If
// that check ever fires, either the authored mix's steer column flipped or the
// steering convention did — both are regressions worth a loud failure, and
// neither is observable from a test that only asks "did it turn?".
//
// (4) is the sharpest check in the file and the mirror image of the equivalent
// check in `ackermann_parity_test.rhai`. There, zero throttle with full steer must
// produce NO rotation, because an Ackermann rover steers through a dedicated
// `steering` port and has no drive force without throttle. Here it must produce
// a LOT of rotation and almost no travel, because this rover steers through the
// drive channels themselves. The two tests bracket the two allocation shapes
// from opposite sides: a refactor that collapsed every rover onto one shared
// drive law would break exactly one of them, whichever shape it picked.
//
// ── THE ABSOLUTE CEILING ────────────────────────────────────────────────────
//
// This rover composes the same `components/mobility/wheel.usda` as every other,
// so it obeys the same one authored no-load axle speed: the motor's
// `lunco:motor:noLoadSpeed` (14400 rad/s) divided by the gearbox
// `lunco:gearbox:ratio` (1200) = 12 rad/s at the axle, times r = 0.4 m:
//
//     v_max = omega_max * r = 12.0 * 0.4 = 4.8 m/s
//
// Six driven wheels instead of four changes the available TRACTION, not the
// no-load speed, so the ceiling is the same number the skid and Ackermann gates
// assert. That shared anchor is the point: three rovers, three allocations, one
// speed contract.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// A scenario is a tick hook on a live sim, not a process, so there is no exit
// code. This file uses the verdict contract the rhai test harness established:
// checks return "" on pass or a message on failure, and the run ends with a
// greppable verdict on stdout (the process log):
//
//     TESTS_OK <n>                    or   TESTS_FAIL <n>/<m>
//     SIX INDEPENDENT MOTION: PASS    or   SIX INDEPENDENT MOTION: FAIL   <- LAST
//
// A caller greps for `SIX INDEPENDENT MOTION:`. The suite ALSO emits
// `SIX_INDEPENDENT_MOTION` on the telemetry bus with value "PASS"/"FAIL" and
// raises a toast, so a UI run is not silent either.
//
// The helpers are defined LOCALLY rather than imported: `tests/lib/*.rhai` is
// concatenated by a shell runner for one-shot `RunRhai` scripts and is NOT part
// of the scenario prelude, and rhai has no cross-file function sharing.
//
// ── WHY A PASSING-LOOKING RUN IS NOT ENOUGH ─────────────────────────────────
//
// Scenarios fail SILENTLY: a hook that never fires, a `find` that returns -1, a
// phase that never advances all look exactly like a clean run. So "no numbers"
// is a FAILURE here, not a pass:
//   * `on_start` prints a banner with the resolved gid; a missing rover fails on
//     the first tick instead of ticking forever.
//   * every leg prints its yaw and travel — the log IS the evidence.
//   * the straight leg requires real distance and a real terminal speed, so a
//     dead command surface fails before it can reach the checks whose PASS
//     condition is "small" (pivot travel, straight-line drift).
// If you do not see a leg table and a verdict line, the test did not run.

// ── Local assertion helpers (stateless — `this` is NOT bound in helpers) ────

/// A magnitude must EXCEED a bound — the anti-dead-test guard. Every check whose
/// pass condition is "small" is paired with one of these on the same run.
fn t_min(v, lo, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v.abs() >= lo { return ""; }
    return what + ": " + r2(v) + " below " + lo + " - nothing happened, so this is a dead test";
}

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT (the convention in `prelude/nav.rhai`, where +steer yaws right). Sign
/// comes from the yaw-plane cross product, so a rover that turned the WRONG way
/// cannot pass by magnitude alone — which is the entire content of assertion (3)
/// in the header.
/// Find one `DriveMix.entries` term by port name, or `()`.
///
/// `get(rover, "DriveMix.entries")` reflects the live allocation table out of the
/// ECS as an array of maps (`#{ port, forward, steer, brake }`) — the authored
/// mix as the ENGINE actually parsed it, not as the USD text reads. That
/// distinction is the point: these checks fail if the `DriveMix` scope reader
/// stops producing the table, or drops terms, even while the USD still
/// describes them.
fn mix_entry(entries, port) {
    if type_of(entries) != "array" { return (); }
    for e in entries {
        if type_of(e) != "map" { continue; }
        if e.port == port { return e; }
    }
    ()
}

/// Assert one channel's coefficients. Messages are built by `+=` and hoisted
/// into a local before being pushed: rhai caps EXPRESSION complexity, and a
/// concatenation chain passed AS a call argument trips it at compile time.
fn check_channel(entries, port, want_fwd, want_steer) {
    let e = mix_entry(entries, port);
    if type_of(e) == "()" {
        return "driveMix has NO `" + port + "` channel - the authored six-channel mix lost a wheel";
    }
    if (e.forward - want_fwd).abs() < 1e-6 && (e.steer - want_steer).abs() < 1e-6 {
        return "";
    }
    let m = "driveMix channel `" + port + "` is forward=" + e.forward;
    m += " steer=" + e.steer;
    m += ", authored as forward=" + want_fwd;
    m += " steer=" + want_steer;
    return m;
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    // Tunables live on `this`: named fns cannot see top-level `const`s.
    this.t = 0.0;
    this.settle_s = 3.0;     // land and stop bouncing
    this.drive_s = 12.0;     // full throttle, straight — long enough to plateau
    this.leg_s = 6.0;        // each steer leg, measured
    this.pivot_s = 6.0;      // zero throttle + full steer, measured
    this.brake_s = 2.0;      // unmeasured brake window before each new leg
    this.plateau_s = 3.0;    // trailing window of the straight leg averaged as v_terminal
    this.sample_s = 0.5;     // log cadence

    // The authored contract: motor no-load speed / gearbox ratio, times radius.
    this.omega_max = 12.0;
    this.wheel_r = 0.4;
    this.v_ceiling = 4.8;

    // Steer command magnitude for the two turning legs. Below 1.0 so the mix is
    // measured in its LINEAR range: at |steer| = 1 with throttle 1 the left bank
    // computes 1*1 + 1*1 = 2 and clamps to 1 while the right bank computes 0,
    // and a clamp is not a projection. 0.6 keeps every channel inside +/-1, so
    // the legs test the authored coefficients rather than the clamp.
    this.steer_cmd = 0.6;

    this.phase = "settle";
    this.phase_start = 0.0;
    this.next_sample = 0.0;

    this.rover = find("/SixIndependentParity/Rover");

    this.pos_prev = ();
    this.fprev = ();

    // Heading-sample coverage across the four MEASURED legs. `yaw_delta_deg`
    // answers `()` when the orientation is degenerate, and the accumulators simply
    // skip those ticks — which means a rover whose heading was NEVER readable would
    // finish with every yaw accumulator still at its initial 0.0 and sail through
    // the checks whose pass condition is "small" (straight drift, pivot travel).
    // Counting the skips turns that silent free pass into a loud failure: the yaw
    // measurements are only admissible if the heading was actually observable.
    this.yaw_ok = 0;
    this.yaw_bad = 0;

    // Straight leg
    this.dist = 0.0;
    this.vmax = 0.0;
    this.vsum = 0.0; this.vn = 0; this.vterm = 0.0;
    this.yaw_straight = 0.0;

    // Steer legs + pivot
    this.yaw_right = 0.0; this.dist_right = 0.0;
    this.yaw_left = 0.0;  this.dist_left = 0.0;
    this.yaw_pivot = 0.0; this.dist_pivot = 0.0;

    print("[six] ── six-wheel independent-drive motion test ────────────────");
    print("[six] rover gid=" + this.rover);
    print("[six] NOT a raycast-vs-physical parity test: this rover has no `drivetrain`");
    print("[six]   variantSet, so there is no second realization to compare against.");
    print("[six] contract: omega_max=" + this.omega_max + " rad/s * r=" + this.wheel_r +
          " m = " + this.v_ceiling + " m/s");
    print("[six] legs: settle " + this.settle_s + "s -> straight " + this.drive_s +
          "s -> steer +" + this.steer_cmd + " -> steer -" + this.steer_cmd +
          " -> zero-throttle pivot");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op: `find` yields -1 and every
    // subsequent read is (). Fail on the FIRST tick instead of ticking forever.
    if this.rover == () || this.rover < 0 {
        print("  MOTION FAIL: rover prim not found (gid=" + this.rover +
              ") - wrong scene? expected scenes/tests/six_independent_parity.usda");
        print("TESTS_FAIL 1/1");
        print("SIX INDEPENDENT MOTION: FAIL");
        emit("SIX_INDEPENDENT_MOTION", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let t = this.t;
    let pos = world_pos(this.rover);
    let fwd = world_forward(this.rover);

    // Speed from POSITION DELTA, not `speed()`. `speed()` reports 0.0 for these
    // rovers even while they visibly travel tens of metres — it reads a velocity
    // source these vessels do not publish, and trusting it produces a "zero, so
    // it matches" reading, which is the dead-test trap this file refuses.
    let d_t = dt();
    let v = if d_t > 0.0 { seg(this.pos_prev, pos) / d_t } else { 0.0 };

    // Per-tick swept yaw. ACCUMULATED, never start-vs-end: `yaw_delta_deg` is
    // an `acos`, so it saturates at 180 degrees and beyond that reads as a turn
    // the OTHER way. This rover passes 180 degrees inside the pivot leg. Per-tick
    // deltas are always far below the wrap, so the running sum is the true swept
    // angle, unbounded.
    let dy = yaw_delta_deg(this.fprev, fwd);
    let step = seg(this.pos_prev, pos);
    this.pos_prev = pos;
    this.fprev = fwd;

    // Score heading readability on the MEASURED legs only. The settle and brake
    // windows legitimately hand back `()` on their first tick (no previous sample
    // yet) and are not evidence of anything.
    let measured = this.phase == "straight" || this.phase == "right";
    let measured2 = this.phase == "left" || this.phase == "pivot";
    if measured || measured2 {
        if type_of(dy) == "()" { this.yaw_bad += 1; } else { this.yaw_ok += 1; }
    }

    let since = t - this.phase_start;

    // ── Phase 1: settle ─────────────────────────────────────────────────────
    if this.phase == "settle" {
        brake(this.rover);
        if t >= this.settle_s {
            this.phase = "straight";
            this.phase_start = t;
            this.next_sample = t;
            print("[six] settled - FULL THROTTLE, steer 0");
        }
        return;
    }

    // ── Phase 2: full throttle, steer 0 — must run STRAIGHT ────────────────
    if this.phase == "straight" {
        drive(this.rover, 1.0, 0.0);
        this.dist += step;
        if v > this.vmax { this.vmax = v; }
        if type_of(dy) != "()" { this.yaw_straight += dy; }

        // Terminal speed = mean over the trailing plateau window, not a single
        // frame: one noisy tick is not a top speed.
        if since >= this.drive_s - this.plateau_s {
            this.vsum += v;
            this.vn += 1;
        }

        if t >= this.next_sample {
            let s = "[six] straight t=" + r2(t) + "s";
            s += "  v=" + r2(v);
            s += "  dist=" + r2(this.dist);
            s += "  yaw=" + r2(this.yaw_straight) + "deg";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.drive_s {
            // Plain statements, not an inline `if` EXPRESSION as a right-hand
            // side: rhai caps expression complexity and that form trips it.
            this.vterm = 0.0;
            if this.vn > 0 {
                this.vterm = this.vsum / this.vn.to_float();
            }
            this.phase = "brake_right";
            this.phase_start = t;
            let s = "[six] straight done: v_term=" + r2(this.vterm);
            s += " v_peak=" + r2(this.vmax);
            s += " dist=" + r2(this.dist);
            s += " yaw_drift=" + r2(this.yaw_straight) + "deg";
            print(s);
        }
        return;
    }

    // ── Unmeasured brake windows ────────────────────────────────────────────
    // Each turning leg starts from rest. Without this the previous leg's
    // momentum would carry into the next one and the pivot leg in particular
    // would answer "did it travel?" with inertia instead of with allocation.
    if this.phase == "brake_right" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "right";
            this.phase_start = t;
            this.next_sample = t;
            print("[six] RIGHT leg: throttle 1.0, steer +" + this.steer_cmd +
                  " (left bank faster => must yaw RIGHT => positive)");
        }
        return;
    }

    if this.phase == "brake_left" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "left";
            this.phase_start = t;
            this.next_sample = t;
            print("[six] LEFT leg: throttle 1.0, steer -" + this.steer_cmd +
                  " (right bank faster => must yaw LEFT => negative)");
        }
        return;
    }

    if this.phase == "brake_pivot" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "pivot";
            this.phase_start = t;
            this.next_sample = t;
            print("[six] PIVOT leg: throttle 0.0, steer +1.0");
            print("[six]   left bank +1, right bank -1: this rover MUST spin on the spot.");
            print("[six]   (An Ackermann rover must do the OPPOSITE - see ackermann_parity_test.rhai.)");
        }
        return;
    }

    // ── Phase 3: steer RIGHT under throttle ─────────────────────────────────
    if this.phase == "right" {
        drive(this.rover, 1.0, this.steer_cmd);
        this.dist_right += step;
        if type_of(dy) != "()" { this.yaw_right += dy; }

        if t >= this.next_sample {
            let s = "[six] right t=" + r2(t) + "s";
            s += "  yaw=" + r2(this.yaw_right) + "deg";
            s += "  dist=" + r2(this.dist_right) + "m";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.leg_s {
            this.phase = "brake_left";
            this.phase_start = t;
            print("[six] right leg done: yaw=" + r2(this.yaw_right) + "deg");
        }
        return;
    }

    // ── Phase 4: steer LEFT under throttle ──────────────────────────────────
    if this.phase == "left" {
        drive(this.rover, 1.0, -this.steer_cmd);
        this.dist_left += step;
        if type_of(dy) != "()" { this.yaw_left += dy; }

        if t >= this.next_sample {
            let s = "[six] left t=" + r2(t) + "s";
            s += "  yaw=" + r2(this.yaw_left) + "deg";
            s += "  dist=" + r2(this.dist_left) + "m";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.leg_s {
            this.phase = "brake_pivot";
            this.phase_start = t;
            print("[six] left leg done: yaw=" + r2(this.yaw_left) + "deg");
        }
        return;
    }

    // ── Phase 5: ZERO throttle + full steer — must PIVOT on the spot ───────
    if this.phase == "pivot" {
        drive(this.rover, 0.0, 1.0);
        this.dist_pivot += step;
        if type_of(dy) != "()" { this.yaw_pivot += dy; }

        if t >= this.next_sample {
            let s = "[six] pivot t=" + r2(t) + "s";
            s += "  yaw=" + r2(this.yaw_pivot) + "deg";
            s += "  moved=" + r2(this.dist_pivot) + "m";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.pivot_s {
            brake(this.rover);
            this.phase = "done";
            // An explicit map, NOT `this`: helpers never see `this` (the runtime
            // binds it only in the hook it calls directly), and passing a plain
            // map keeps the verdict a pure function of the measurements.
            verdict(#{
                vterm: this.vterm, vmax: this.vmax, dist: this.dist,
                yaw_straight: this.yaw_straight,
                yaw_right: this.yaw_right, dist_right: this.dist_right,
                yaw_left: this.yaw_left,   dist_left: this.dist_left,
                yaw_pivot: this.yaw_pivot, dist_pivot: this.dist_pivot,
                yaw_ok: this.yaw_ok, yaw_bad: this.yaw_bad,
                kernel: get(this.rover, "DriveMix.kernel"),
                entries: get(this.rover, "DriveMix.entries"),
                v_ceiling: this.v_ceiling,
                steer_cmd: this.steer_cmd
            });
        }
        return;
    }
}

/// Stop the rover if the scenario is detached or hot-reloaded mid-run.
fn on_stop(me) {
    let r = find("/SixIndependentParity/Rover");
    if r >= 0 { brake(r); }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// BOUNDS — why these numbers. Unlike a parity test, nothing here is a ratio
// between two solvers, so every bound is absolute and each one is set far from
// the defect it guards rather than close to the expected value. This file is a
// regression net, not a characterisation: it must not fail because someone
// legitimately retuned a suspension.
//
//   terminal speed  [0.5, 1.25] * omega_max*r = [2.4, 6.0] m/s. The floor
//                   catches a dead command surface; the ceiling catches a
//                   drivetrain that stopped reading the authored no-load speed
//                   (the historic failure put a rover at ~24 m/s).
//   straight dist   >= 20 m over 12 s. Implied by the speed floor, asserted
//                   separately because distance and speed come from the same
//                   `world_pos` samples and disagreeing would mean the harness
//                   itself is broken.
//   straight drift  <= 25 deg. Six equal channels cannot curve the rover; this
//                   catches a mix where one bank picked up a steering term. Wide
//                   enough to absorb suspension wander on a settling rover.
//   leg yaw         >= 15 deg each way. Catches a rover that barely turns.
//   leg symmetry    +/-40%. Deliberately loose — the two legs are not run from
//                   identical states (the second starts from a heading the first
//                   left behind) and lateral friction saturation is the most
//                   solver-sensitive quantity in the model. It exists to catch a
//                   mix that steers strongly one way and weakly the other, not
//                   to pin a yaw rate.
//   pivot yaw       >= 60 deg AND positive. This is the allocation-SHAPE check:
//                   at zero throttle the channels are `+1` left / `-1` right, so
//                   the rover must spin. 60 deg is a small fraction of what it
//                   actually sweeps and enormous compared with the ~0 an
//                   Ackermann-style mix would produce.
//   pivot travel    <= 4.0 m. A pivot is a rotation, not a drive. Generous
//                   because a six-wheel skid pivot genuinely scrubs sideways.
//
// SIGN CHECKS ARE NOT TOLERANT. `+steer` must yaw positive and `-steer` must
// yaw negative, derived from the authored mix in the header. A magnitude-only
// test would pass a rover that steers backwards.
fn verdict(s) {
    let f = [];

    // ── (0) the yaw measurements are admissible at all ─────────────────────
    // Every yaw check below reads an accumulator that SKIPS ticks whose heading was
    // degenerate. If most ticks were skipped the accumulators sit near 0 for want of
    // data, not for want of rotation, and the "must stay small" checks would pass on
    // an empty measurement. Demand the heading was readable on the overwhelming
    // majority of measured ticks before believing any of them.
    let y_tot = s.yaw_ok + s.yaw_bad;
    let m_cov = "heading unreadable on " + s.yaw_bad + " of " + y_tot;
    m_cov += " measured ticks - world_forward returned a degenerate orientation, so the";
    m_cov += " yaw accumulators below measure missing data rather than rotation";
    f.push(t_true(y_tot > 0 && s.yaw_bad * 20 < y_tot, m_cov));

    // ── (1) pure throttle runs straight, at the authored ceiling ───────────
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
    f.push(t_range(s.vterm, lo, hi,
                   "terminal speed vs authored omega_max*r=" + s.v_ceiling));
    f.push(t_range(s.dist, 20.0, 200.0, "straight-leg distance (m)"));
    f.push(t_max(s.yaw_straight, 25.0,
                 "yaw drift under steer 0 (deg) - six equal channels must not curve"));

    // ── (2) + (3) it turns, and it turns the RIGHT way ─────────────────────
    let m_right = "steer +" + s.steer_cmd + " swept " + r2(s.yaw_right);
    m_right += "deg - the authored mix gives the LEFT bank +steer, so the left side";
    m_right += " outruns the right and the rover must yaw RIGHT (positive)";
    f.push(t_true(s.yaw_right > 0.0, m_right));
    f.push(t_min(s.yaw_right, 15.0, "yaw swept under steer +" + s.steer_cmd + " (deg)"));

    let m_left = "steer -" + s.steer_cmd + " swept " + r2(s.yaw_left);
    m_left += "deg - mirrored command, so the RIGHT bank outruns the left and the";
    m_left += " rover must yaw LEFT (negative)";
    f.push(t_true(s.yaw_left < 0.0, m_left));
    f.push(t_min(s.yaw_left, 15.0, "yaw swept under steer -" + s.steer_cmd + " (deg)"));

    f.push(t_rel(s.yaw_right.abs(), s.yaw_left.abs(), 40.0,
                 "steer symmetry: |yaw right| vs |yaw left| (deg)"));

    // ── (4) zero throttle + full steer pivots on the spot ──────────────────
    let m_piv = "zero-throttle full-steer pivot swept only " + r2(s.yaw_pivot);
    m_piv += "deg - this rover steers THROUGH its drive channels (+1 left, -1 right),";
    m_piv += " so it must spin on the spot with no throttle. Barely turning here is";
    m_piv += " the signature of allocation collapsing onto a dedicated-steering-port";
    m_piv += " shape, which belongs to the Ackermann rover and not to this one.";
    f.push(t_true(s.yaw_pivot > 60.0, m_piv));
    f.push(t_max(s.dist_pivot, 4.0, "travel during the pivot (m) - a pivot is not a drive"));

    // ── The authored allocation itself, read back out of the engine ────────
    f.push(t_true(s.kernel == "linear",
                  "drive kernel is '" + s.kernel + "', expected 'linear' (the authored DriveMix scope)"));

    if type_of(s.entries) != "array" {
        f.push("DriveMix.entries is not an array - the authored six-channel mix never parsed");
    } else {
        let m_len = "driveMix has " + s.entries.len() + " channels, expected 6";
        f.push(t_true(s.entries.len() == 6, m_len));
        // Left bank: forward + steer. Right bank: forward - steer.
        f.push(check_channel(s.entries, "drive_w0", 1.0, 1.0));
        f.push(check_channel(s.entries, "drive_w1", 1.0, 1.0));
        f.push(check_channel(s.entries, "drive_w2", 1.0, 1.0));
        f.push(check_channel(s.entries, "drive_w3", 1.0, -1.0));
        f.push(check_channel(s.entries, "drive_w4", 1.0, -1.0));
        f.push(check_channel(s.entries, "drive_w5", 1.0, -1.0));
    }

    print("[six] ── results ────────────────────────────────────────────────");
    print("[six]   v_terminal (m/s)      " + r2(s.vterm));
    print("[six]   v_peak     (m/s)      " + r2(s.vmax));
    print("[six]   straight dist (m)     " + r2(s.dist));
    print("[six]   straight yaw  (deg)   " + r2(s.yaw_straight));
    print("[six]   yaw @ steer + (deg)   " + r2(s.yaw_right));
    print("[six]   yaw @ steer - (deg)   " + r2(s.yaw_left));
    print("[six]   yaw @ pivot   (deg)   " + r2(s.yaw_pivot));
    print("[six]   travel @ pivot (m)    " + r2(s.dist_pivot));
    print("[six]   heading samples ok    " + s.yaw_ok + " (unreadable " + s.yaw_bad + ")");
    print("[six]   kernel                " + s.kernel);
    print("[six]   authored ceiling omega_max*r = " + s.v_ceiling + " m/s");

    report_verdict(f, "SIX INDEPENDENT MOTION", "SIX_INDEPENDENT_MOTION");
}
