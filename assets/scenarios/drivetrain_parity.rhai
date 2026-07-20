// ── Drivetrain parity test — raycast wheels vs joint wheels ─────────────────
//
// TEST-ONLY. Runs against `scenes/sandbox/drivetrain_parity.usda`, which puts
// the SAME rover asset on the SAME ground twice, differing in exactly one
// authored opinion: `variants = { string drivetrain = "raycast" | "physical" }`.
// This script drives BOTH from ONE tick loop — same command, same frame — and
// asserts they behave the same.
//
// WHAT IT GUARDS. Raycast and joint wheels are two REALIZATIONS of ONE parameter
// set. They diverged once: the no-load axle speed was authored under two names
// (60 vs 12 rad/s) AND the raycast drive force had no torque-speed term at all,
// so a raycast rover ran ~5x faster than a joint rover built from the same file.
// Both now read the single authored `physxVehicleEngine:maxRotationSpeed`
// (12.0 rad/s, `components/mobility/wheel.usda`): the joint motor targets it,
// the raycast drive force rolls off toward it. With r = 0.4 m both must cap at
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

/// 2 dp, for the log table.
fn r2(x) { (x * 100.0).round() / 100.0 }

/// RELATIVE-tolerance compare, as a percentage of the larger magnitude.
///
/// Relative (not absolute, unlike `t_near` in the shared harness) because the
/// quantity under test is a RATIO between two solvers: the historic bug was 5x,
/// and "within 15% of each other" is the honest statement of parity at any
/// speed. Two values that are both ~0 are a FAILURE, not a pass — that is the
/// dead-command-surface case, where nothing moved and everything "matched".
fn t_rel(a, b, tol_pct, what) {
    if type_of(a) == "()" || type_of(b) == "()" {
        return what + ": MISSING (raycast=" + a + " physical=" + b + ")";
    }
    let m = if a.abs() > b.abs() { a.abs() } else { b.abs() };
    if m < 0.001 {
        return what + ": BOTH ~ZERO (raycast=" + r2(a) + " physical=" + r2(b) +
               ") - nothing moved, so this is a dead test, not a match";
    }
    let d = (a - b).abs() / m * 100.0;
    if d <= tol_pct { return ""; }
    return what + ": raycast=" + r2(a) + " physical=" + r2(b) +
           " ratio=" + r2(a / b) + "x diff=" + r2(d) + "% (tol " + tol_pct + "%)";
}

/// Plain predicate check.
fn t_true(cond, what) { if cond { "" } else { what } }

/// A value must sit within [lo, hi]. Used for the ABSOLUTE ceiling: parity with
/// each other is necessary but not sufficient — both could drift together.
fn t_range(v, lo, hi, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v >= lo && v <= hi { return ""; }
    return what + ": " + r2(v) + " outside [" + lo + ", " + hi + "]";
}

/// Print the failures and the verdict. Call ONCE, last. Returns true on pass.
/// The LAST line printed is `DRIVETRAIN PARITY: PASS|FAIL` — the greppable one.
fn t_report(fails) {
    let real = [];
    for f in fails { if f != "" { real.push(f); } }
    let total = fails.len();
    if real.is_empty() {
        print("TESTS_OK " + total);
        print("DRIVETRAIN PARITY: PASS");
        return true;
    }
    for f in real { print("  PARITY FAIL: " + f); }
    print("TESTS_FAIL " + real.len() + "/" + total);
    print("DRIVETRAIN PARITY: FAIL");
    false
}

/// Path length of one tick's motion, 0 when there is no previous sample.
fn seg(prev, cur) {
    if prev == () || cur == () { return 0.0; }
    vlen(vsub(cur, prev))
}

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT (matching the skid-drive sign convention in `prelude/nav.rhai`, where
/// +steer yaws right). Sign is taken from the yaw-plane cross product, so a
/// rover that turned the WRONG way cannot pass by magnitude alone.
fn yaw_change_deg(f0, f1) {
    if f0 == () || f1 == () { return (); }
    let a = vnorm(f0);
    let b = vnorm(f1);
    let mag = clamp(vdot(a, b), -1.0, 1.0).acos() * 57.29577951308232;
    // `acos` can still hand back NaN at the domain edge (two identical headings
    // give a dot of 1±1ulp, and the clamp is applied to a value that may already
    // have rounded past it). One NaN is unrecoverable once the caller SUMS these
    // — it poisons the accumulator for the rest of the run and every downstream
    // comparison reads NaN — so a non-finite result is reported as "no rotation
    // measured this tick" rather than propagated. NaN is the only value that is
    // not equal to itself, which is the portable way to ask.
    if mag != mag { return 0.0; }
    let cy = vcross(a, b)[1];
    if cy > 0.0 { -mag } else { mag }
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

// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    // Tunables live on `this`: named fns cannot see top-level `const`s.
    this.t = 0.0;
    this.settle_s = 3.0;    // let both rovers land and stop bouncing
    this.drive_s = 12.0;    // full throttle, straight — long enough to plateau
    this.steer_s = 6.0;     // full throttle + steady steer — yaw authority
    this.plateau_s = 3.0;   // trailing window of the drive phase averaged as v_terminal
    this.sample_s = 0.5;    // log cadence

    // The authored contract under test. omega_max is
    // `physxVehicleEngine:maxRotationSpeed` in components/mobility/wheel.usda.
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
    this.f0_rc = ();   this.f0_ph = ();
    // Accumulated swept yaw (degrees) + the previous tick's forward vector the
    // accumulation differences against. Start at 0.0, not `()`: the steer phase
    // SUMS into these.
    this.yaw_rc = 0.0;  this.yaw_ph = 0.0;
    this.fprev_rc = (); this.fprev_ph = ();

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
              "scenes/sandbox/drivetrain_parity.usda");
        print("TESTS_FAIL 1/1");
        print("DRIVETRAIN PARITY: FAIL");
        emit("DRIVETRAIN_PARITY", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let t = this.t;
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

        this.d_rc += seg(this.p_rc, pos_rc);
        this.d_ph += seg(this.p_ph, pos_ph);
        this.p_rc = pos_rc;
        this.p_ph = pos_ph;
        if v_rc > this.vmax_rc { this.vmax_rc = v_rc; }
        if v_ph > this.vmax_ph { this.vmax_ph = v_ph; }

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
        // `yaw_change_deg` is an `acos`, so it saturates at 180° and then reads
        // as a turn the OTHER way. A skid rover at full steer passes 180° well
        // inside this phase — the first run logged raycast going 170.9° then
        // −164.8° and the test called it "steered OPPOSITE ways" when both were
        // turning the same direction. Per-tick deltas are always far below the
        // wrap, so the running sum is the true swept angle, unbounded.
        let f_rc = world_forward(this.rc);
        let f_ph = world_forward(this.ph);
        let dy_rc = yaw_change_deg(this.fprev_rc, f_rc);
        let dy_ph = yaw_change_deg(this.fprev_ph, f_ph);
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
                v_ceiling: this.v_ceiling
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
//   yaw change      +/-35%  Skid-steer yaw comes from lateral friction saturation,
//                           the most solver-sensitive quantity here. Deliberately
//                           loose: this check exists to catch a rover that barely
//                           turns or turns the WRONG WAY, not to pin a yaw rate.
//                           The SIGN is checked separately and is NOT tolerant.
//
// The ABSOLUTE ceiling is the real anchor. Parity alone is satisfiable by both
// rovers being wrong together, so both must also land in
// [0.5, 1.25] * omega_max * r = [2.4, 6.0] m/s. The historic bug put the raycast
// rover at ~24 m/s; the floor catches a dead command surface.
fn verdict(s) {
    let f = [];

    f.push(t_rel(s.vterm_rc, s.vterm_ph, 15.0, "terminal speed (m/s)"));
    f.push(t_rel(s.vmax_rc, s.vmax_ph, 15.0, "peak speed (m/s)"));
    f.push(t_rel(s.d_rc, s.d_ph, 20.0, "distance travelled (m)"));

    // Absolute ceiling + floor: both must honour omega_max * r, not merely match.
    let lo = s.v_ceiling * 0.5;
    let hi = s.v_ceiling * 1.25;
    f.push(t_range(s.vterm_rc, lo, hi,
                   "raycast terminal speed vs authored omega_max*r=" + s.v_ceiling));
    f.push(t_range(s.vterm_ph, lo, hi,
                   "physical terminal speed vs authored omega_max*r=" + s.v_ceiling));

    // Yaw: direction first (intolerant), then magnitude (loose).
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
        f.push(t_rel(s.yaw_rc, s.yaw_ph, 35.0, "heading change (deg)"));
    }

    print("[parity] ── results ────────────────────────────────────────────");
    print("[parity]   quantity          raycast     physical");
    print("[parity]   v_terminal (m/s)  " + r2(s.vterm_rc) + "       " + r2(s.vterm_ph));
    print("[parity]   v_peak     (m/s)  " + r2(s.vmax_rc) + "       " + r2(s.vmax_ph));
    print("[parity]   distance   (m)    " + r2(s.d_rc) + "       " + r2(s.d_ph));
    print("[parity]   yaw        (deg)  " + s.yaw_rc + "       " + s.yaw_ph);
    print("[parity]   authored ceiling omega_max*r = " + s.v_ceiling + " m/s");

    let ok = t_report(f);
    emit("DRIVETRAIN_PARITY", if ok { "PASS" } else { "FAIL" });
    if ok {
        notify_kind("Drivetrain parity: PASS", "success");
    } else {
        notify_kind("Drivetrain parity: FAIL - raycast and joint wheels diverged", "error");
    }
}
