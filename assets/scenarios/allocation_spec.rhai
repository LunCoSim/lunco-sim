// ── Drive-allocation SPEC — the scenario-level replacement for the kernel unit tests ──
//
// TEST-ONLY. Runs against `scenes/sandbox/drivetrain_parity.usda` (it uses that
// scene's `RoverRaycast` — a skid rover with the raycast drivetrain). It is not
// attached to that scene's USD, because that scene already owns a `LunCoProgram`
// for its own gate; run this one explicitly against the loaded scene:
//
//     load_scene  scenes/sandbox/drivetrain_parity.usda
//     run_scenario scenarios/allocation_spec.rhai
//
// ── WHY THIS FILE EXISTS ────────────────────────────────────────────────────
//
// `crates/lunco-mobility/src/kernels.rs` carries five unit tests that are the
// ONLY executable statement of what rover drive allocation MEANS:
//
//   1. `skid_steer_priority_lets_the_inner_side_counter_rotate`
//   2. `skid_reverse_mirrors_forward_at_zero_steer`
//   3. `skid_kernel_maps_to_named_ports`
//   4. `linear_kernel_projects_each_entry`
//   5. `registry_defaults_resolve_by_name`
//
// They are unit tests over pure functions, so they die with the functions. When
// allocation moves out of Rust and into authored programs, the maths survives
// but its spec does not — and a spec that only existed as `assert!` lines next
// to the code it described leaves nothing behind to notice that the new
// implementation allocates differently.
//
// This file re-states those assertions as OBSERVABLE CONSEQUENCES on a live
// rover, so they keep holding whoever computes them. That reframing is not free,
// and the losses are recorded honestly below rather than papered over.
//
// ── WHAT IS COVERED HERE, AND WHAT IS NOT ───────────────────────────────────
//
//   (1) steer priority / inner-side counter-rotation — PARTIALLY covered.
//       The unit test pins exact numbers: `skid_mix_norm(1.0, 1.0)` gives
//       `l = 1.0`, `r = -1/3`, the inner side NEGATIVE rather than clamped to 0.
//       At zero throttle with full steer the mix is an unambiguous `+1 / -1` and
//       the counter-rotation IS directly observable — the PIVOT leg below reads
//       it straight off the two banks' wheel spin and asserts opposite signs.
//       Under THROTTLE, though, it is not: a wheel commanded to -1/3 while the
//       rover is travelling forward at 4.8 m/s is dragged forward by the ground
//       regardless of its command, so `spin_velocity` reports the ground, not
//       the allocation. The exact -1/3 is therefore NOT observable from outside
//       the kernel. What IS observable is the PROPERTY the -1/3 exists to
//       deliver — the thing the unit test's own comment names, "otherwise
//       steering becomes a lazy arc" — so the PRIORITY leg asserts sharp yaw
//       authority at FULL throttle. A mix that clamped the inner side to zero
//       would still turn, but far more slowly.
//
//   (2) reverse mirrors forward at zero steer — COVERED, as motion. Equal
//       distance, opposite direction, and both banks' spin sign flipped.
//
//   (3) skid maps to NAMED ports — COVERED exactly. `DriveMix.ports` is
//       reflected out of the live ECS and compared against
//       `["drive_left", "drive_right"]`.
//
//   (4) the linear mix projects each entry — NOT covered here, because this
//       scene's rover is a SKID rover and has no linear mix to project. It is
//       covered in `six_independent_parity.rhai`, which reads the six-channel
//       `DriveMix.entries` back out of the engine and asserts each channel's
//       coefficients alongside the motion they produce.
//
//   (5) the registry resolves kernels by name — PARTIALLY covered. That the
//       name `"skid"` resolves to a WORKING kernel is asserted here twice over:
//       `DriveMix.kernel` reads back as `"skid"`, and the rover then actually
//       drives, which it cannot do through an unresolved name (`apply_drive_mix`
//       leaves an unknown kernel un-actuated — a fail-safe coast). What is NOT
//       observable at scenario level is the NEGATIVE half of the unit test,
//       `reg.get("nope").is_none()`: there is no way to author a vessel with a
//       bogus kernel name from inside a running scenario and watch the lookup
//       miss. That assertion has no scenario-level equivalent and is lost.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// A scenario is a tick hook on a live sim, not a process, so there is no exit
// code. This file uses the verdict contract the rhai test harness established:
// checks return "" on pass or a message on failure, and the run ends with a
// greppable verdict on stdout (the process log):
//
//     TESTS_OK <n>              or   TESTS_FAIL <n>/<m>
//     ALLOCATION SPEC: PASS     or   ALLOCATION SPEC: FAIL       <- LAST line
//
// A caller greps for `ALLOCATION SPEC:`. The suite ALSO emits `ALLOCATION_SPEC`
// on the telemetry bus with value "PASS"/"FAIL" and raises a toast.
//
// The helpers are defined LOCALLY rather than imported: `tests/lib/*.rhai` is
// concatenated by a shell runner for one-shot `RunRhai` scripts and is NOT part
// of the scenario prelude, and rhai has no cross-file function sharing.
//
// ── WHY A PASSING-LOOKING RUN IS NOT ENOUGH ─────────────────────────────────
//
// Scenarios fail SILENTLY: a hook that never fires, a `find` that returns -1, a
// phase that never advances all look exactly like a clean run. So "no numbers"
// is a FAILURE here, not a pass. Every check whose pass condition is "small"
// (pivot travel) is paired on the same run with one that demands real motion,
// and the SIGN conventions are never assumed — they are derived from the
// rover's own forward leg, so this file cannot pass by agreeing with itself
// about a convention that flipped.

// ── Local assertion helpers (stateless — `this` is NOT bound in helpers) ────

/// 2 dp, for the log table.
fn r2(x) { (x * 100.0).round() / 100.0 }

/// Plain predicate check.
fn t_true(cond, what) { if cond { "" } else { what } }

/// A magnitude must EXCEED a bound — the anti-dead-test guard.
fn t_min(v, lo, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v.abs() >= lo { return ""; }
    return what + ": " + r2(v) + " below " + lo + " - nothing happened, so this is a dead test";
}

/// A magnitude must stay BELOW a bound. `()` fails explicitly rather than
/// sliding through as "small".
fn t_max(v, hi, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v.abs() <= hi { return ""; }
    return what + ": " + r2(v) + " exceeds " + hi;
}

/// RELATIVE-tolerance compare, as a percentage of the larger magnitude. Two
/// values that are both ~0 are a FAILURE, not a pass — the dead-command-surface
/// case, where nothing moved and everything "matched".
fn t_rel(a, b, tol_pct, what) {
    if type_of(a) == "()" || type_of(b) == "()" {
        return what + ": MISSING (a=" + a + " b=" + b + ")";
    }
    let m = if a.abs() > b.abs() { a.abs() } else { b.abs() };
    if m < 0.001 {
        return what + ": BOTH ~ZERO (a=" + r2(a) + " b=" + r2(b) +
               ") - nothing moved, so this is a dead test, not a match";
    }
    let d = (a - b).abs() / m * 100.0;
    if d <= tol_pct { return ""; }
    return what + ": a=" + r2(a) + " b=" + r2(b) + " diff=" + r2(d) +
           "% (tol " + tol_pct + "%)";
}

/// Two values must have OPPOSITE, non-trivial signs. The direct expression of
/// "the inner side counter-rotates": a wheel bank at rest, or both banks turning
/// the same way, both fail. The `dead` band exists because a bank that is barely
/// creeping has no meaningful sign at all.
fn t_opposite(a, b, dead, what) {
    if type_of(a) == "()" || type_of(b) == "()" {
        return what + ": MISSING (a=" + a + " b=" + b + ")";
    }
    if a.abs() < dead || b.abs() < dead {
        return what + ": one side is not turning (a=" + r2(a) + " b=" + r2(b) +
               ", dead band " + dead + ") - no counter-rotation to measure";
    }
    if (a > 0.0) == (b > 0.0) {
        return what + ": SAME sign (a=" + r2(a) + " b=" + r2(b) +
               ") - the banks are not counter-rotating";
    }
    ""
}

/// Print the failures and the verdict. Call ONCE, last. Returns true on pass.
/// The LAST line printed is `ALLOCATION SPEC: PASS|FAIL` — the greppable one.
fn t_report(fails) {
    let real = [];
    for f in fails { if f != "" { real.push(f); } }
    let total = fails.len();
    if real.is_empty() {
        print("TESTS_OK " + total);
        print("ALLOCATION SPEC: PASS");
        return true;
    }
    for f in real { print("  SPEC FAIL: " + f); }
    print("TESTS_FAIL " + real.len() + "/" + total);
    print("ALLOCATION SPEC: FAIL");
    false
}

/// Path length of one tick's motion, 0 when there is no previous sample.
fn seg(prev, cur) {
    if prev == () || cur == () { return 0.0; }
    vlen(vsub(cur, prev))
}

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT. Sign comes from the yaw-plane cross product.
fn yaw_change_deg(f0, f1) {
    if f0 == () || f1 == () { return (); }
    let a = vnorm(f0);
    let b = vnorm(f1);
    let mag = clamp(vdot(a, b), -1.0, 1.0).acos() * 57.29577951308232;
    // `acos` can hand back NaN at the domain edge (two identical headings give a
    // dot of 1±1ulp, and the clamp is applied to a value that may already have
    // rounded past it). One NaN is unrecoverable once the caller SUMS these — it
    // poisons the accumulator for the rest of the run — so a non-finite result is
    // reported as "no rotation measured this tick" rather than propagated. NaN is
    // the only value that is not equal to itself, the portable way to ask.
    if mag != mag { return 0.0; }
    let cy = vcross(a, b)[1];
    if cy > 0.0 { -mag } else { mag }
}

/// Axle spin rate (rad/s) of one raycast wheel, or `()`.
///
/// `WheelRaycast.spin_velocity` is the wheel's own spin integrator, reflected out
/// of the live ECS. It is the closest thing to a per-port readout a scenario can
/// reach: the actuator ports themselves are child ECS entities with no USD prim
/// and therefore no gid, so `read_port` cannot address them from here. Reading
/// the WHEEL instead is arguably the better test anyway — it asserts that the
/// allocation reached the hardware, not merely that a register was written.
fn wheel_spin(w) {
    if type_of(w) == "()" { return (); }
    if w < 0 { return (); }
    get(w, "WheelRaycast.spin_velocity")
}

/// Mean spin of a bank of wheels, or `()` if none of them read back.
fn bank_spin(a, b) {
    let sa = wheel_spin(a);
    let sb = wheel_spin(b);
    if type_of(sa) == "()" && type_of(sb) == "()" { return (); }
    if type_of(sa) == "()" { return sb; }
    if type_of(sb) == "()" { return sa; }
    (sa + sb) / 2.0
}

/// Do two numbers share a sign? Used to pin the forward/reverse mirror and the
/// pivot's counter-rotation against the rover's OWN measured forward direction,
/// so no global sign convention is assumed anywhere in this file.
fn same_sign(a, b) {
    if type_of(a) == "()" || type_of(b) == "()" { return false; }
    (a > 0.0) == (b > 0.0)
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    // Tunables live on `this`: named fns cannot see top-level `const`s.
    this.t = 0.0;
    this.settle_s = 3.0;
    this.leg_s = 8.0;        // forward and reverse legs
    this.pivot_s = 6.0;      // zero throttle + full steer
    this.priority_s = 5.0;   // full throttle + full steer
    this.brake_s = 2.0;      // unmeasured brake window between legs
    this.sample_s = 0.5;

    this.phase = "settle";
    this.phase_start = 0.0;
    this.next_sample = 0.0;

    this.rover = find("/DrivetrainParity/RoverRaycast");
    // The skid rover's four wheels. FL/RL are the LEFT bank (drive_left), FR/RR
    // the RIGHT bank (drive_right) — the two ports the `skid` kernel writes.
    this.w_fl = find("/DrivetrainParity/RoverRaycast/Wheel_FL");
    this.w_fr = find("/DrivetrainParity/RoverRaycast/Wheel_FR");
    this.w_rl = find("/DrivetrainParity/RoverRaycast/Wheel_RL");
    this.w_rr = find("/DrivetrainParity/RoverRaycast/Wheel_RR");

    this.pos_prev = ();
    this.fprev = ();

    // Forward leg
    this.f0_fwd = ();  this.p0_fwd = ();
    this.d_fwd = 0.0;  this.along_fwd = 0.0;
    this.spin_l_fwd = 0.0; this.spin_r_fwd = 0.0; this.n_fwd = 0;

    // Reverse leg
    this.p0_rev = ();
    this.d_rev = 0.0;  this.along_rev = 0.0;
    this.spin_l_rev = 0.0; this.spin_r_rev = 0.0; this.n_rev = 0;

    // Pivot leg (throttle 0, steer 1)
    this.yaw_pivot = 0.0; this.d_pivot = 0.0;
    this.spin_l_piv = 0.0; this.spin_r_piv = 0.0; this.n_piv = 0;

    // Priority leg (throttle 1, steer 1)
    this.yaw_prio = 0.0;

    print("[spec] ── drive-allocation spec ─────────────────────────────────");
    print("[spec] rover gid=" + this.rover);
    print("[spec] wheels FL=" + this.w_fl + " FR=" + this.w_fr +
          " RL=" + this.w_rl + " RR=" + this.w_rr);
    print("[spec] legs: settle -> forward -> reverse -> zero-throttle pivot -> full-throttle steer");
    print("[spec] NOTE: `linear_kernel_projects_each_entry` is covered by");
    print("[spec]   scenarios/six_independent_parity.rhai, not here - this rover is a SKID rover.");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op: `find` yields -1 and every
    // subsequent read is (). Fail on the FIRST tick instead of ticking forever.
    if this.rover == () || this.rover < 0 {
        print("  SPEC FAIL: rover prim not found (gid=" + this.rover +
              ") - wrong scene? expected scenes/sandbox/drivetrain_parity.usda");
        print("TESTS_FAIL 1/1");
        print("ALLOCATION SPEC: FAIL");
        emit("ALLOCATION_SPEC", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let t = this.t;
    let pos = world_pos(this.rover);
    let fwd = world_forward(this.rover);

    let dy = yaw_change_deg(this.fprev, fwd);
    let step = seg(this.pos_prev, pos);
    this.pos_prev = pos;
    this.fprev = fwd;

    let spin_l = bank_spin(this.w_fl, this.w_rl);
    let spin_r = bank_spin(this.w_fr, this.w_rr);

    let since = t - this.phase_start;

    // ── settle ──────────────────────────────────────────────────────────────
    if this.phase == "settle" {
        brake(this.rover);
        if t >= this.settle_s {
            this.phase = "forward";
            this.phase_start = t;
            this.next_sample = t;
            this.p0_fwd = pos;
            this.f0_fwd = fwd;
            print("[spec] settled - FORWARD leg: throttle +1.0, steer 0");
        }
        return;
    }

    // ── Forward leg: establishes the rover's OWN sign conventions ──────────
    // Everything downstream is compared against this leg rather than against a
    // hardcoded "positive is forward". A convention that flips globally would
    // flip here too and the mirror checks would still hold; a convention that
    // flips for only one command would be caught, which is the interesting case.
    if this.phase == "forward" {
        drive(this.rover, 1.0, 0.0);
        this.d_fwd += step;
        if type_of(spin_l) != "()" && type_of(spin_r) != "()" {
            this.spin_l_fwd += spin_l;
            this.spin_r_fwd += spin_r;
            this.n_fwd += 1;
        }

        if t >= this.next_sample {
            let s = "[spec] fwd t=" + r2(t) + "s";
            s += "  dist=" + r2(this.d_fwd);
            s += "  spin_L=" + r2(spin_l);
            s += "  spin_R=" + r2(spin_r);
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.leg_s {
            // Signed travel along the heading the leg STARTED with — positive
            // means the rover went the way it was pointing.
            this.along_fwd = vdot(vsub(pos, this.p0_fwd), vnorm(this.f0_fwd));
            this.phase = "brake_rev";
            this.phase_start = t;
            let s = "[spec] forward leg done: dist=" + r2(this.d_fwd);
            s += "m along-heading=" + r2(this.along_fwd) + "m";
            print(s);
        }
        return;
    }

    // ── Unmeasured brake windows ────────────────────────────────────────────
    // Each leg starts from rest. Without this the previous leg's momentum
    // answers the next leg's question — most damagingly for the pivot, whose
    // whole assertion is "it did not travel".
    if this.phase == "brake_rev" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "reverse";
            this.phase_start = t;
            this.next_sample = t;
            this.p0_rev = pos;
            print("[spec] REVERSE leg: throttle -1.0, steer 0 (must mirror the forward leg)");
        }
        return;
    }

    if this.phase == "brake_piv" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "pivot";
            this.phase_start = t;
            this.next_sample = t;
            print("[spec] PIVOT leg: throttle 0.0, steer +1.0");
            print("[spec]   skid_mix_norm(0, 1) = (+1, -1): the banks must COUNTER-ROTATE.");
        }
        return;
    }

    if this.phase == "brake_prio" {
        brake(this.rover);
        if since >= this.brake_s {
            this.phase = "priority";
            this.phase_start = t;
            this.next_sample = t;
            print("[spec] PRIORITY leg: throttle 1.0, steer +1.0");
            print("[spec]   steer priority (drive = forward * (1 - 0.5*|steer|)) exists so this");
            print("[spec]   is a SHARP turn and not a lazy arc. Yaw authority is the observable.");
        }
        return;
    }

    // ── Reverse leg: mirrors forward at zero steer ──────────────────────────
    if this.phase == "reverse" {
        drive(this.rover, -1.0, 0.0);
        this.d_rev += step;
        if type_of(spin_l) != "()" && type_of(spin_r) != "()" {
            this.spin_l_rev += spin_l;
            this.spin_r_rev += spin_r;
            this.n_rev += 1;
        }

        if t >= this.next_sample {
            let s = "[spec] rev t=" + r2(t) + "s";
            s += "  dist=" + r2(this.d_rev);
            s += "  spin_L=" + r2(spin_l);
            s += "  spin_R=" + r2(spin_r);
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.leg_s {
            this.along_rev = vdot(vsub(pos, this.p0_rev), vnorm(this.f0_fwd));
            this.phase = "brake_piv";
            this.phase_start = t;
            let s = "[spec] reverse leg done: dist=" + r2(this.d_rev);
            s += "m along-heading=" + r2(this.along_rev) + "m";
            print(s);
        }
        return;
    }

    // ── Pivot leg: the counter-rotation, directly observed ──────────────────
    if this.phase == "pivot" {
        drive(this.rover, 0.0, 1.0);
        this.d_pivot += step;
        if type_of(dy) != "()" { this.yaw_pivot += dy; }
        if type_of(spin_l) != "()" && type_of(spin_r) != "()" {
            this.spin_l_piv += spin_l;
            this.spin_r_piv += spin_r;
            this.n_piv += 1;
        }

        if t >= this.next_sample {
            let s = "[spec] pivot t=" + r2(t) + "s";
            s += "  yaw=" + r2(this.yaw_pivot) + "deg";
            s += "  moved=" + r2(this.d_pivot) + "m";
            s += "  spin_L=" + r2(spin_l);
            s += "  spin_R=" + r2(spin_r);
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.pivot_s {
            this.phase = "brake_prio";
            this.phase_start = t;
            print("[spec] pivot leg done: yaw=" + r2(this.yaw_pivot) +
                  "deg moved=" + r2(this.d_pivot) + "m");
        }
        return;
    }

    // ── Priority leg: sharp steering WHILE driving ─────────────────────────
    if this.phase == "priority" {
        drive(this.rover, 1.0, 1.0);
        if type_of(dy) != "()" { this.yaw_prio += dy; }

        if t >= this.next_sample {
            let s = "[spec] priority t=" + r2(t) + "s";
            s += "  yaw=" + r2(this.yaw_prio) + "deg";
            print(s);
            this.next_sample = t + this.sample_s;
        }

        if since >= this.priority_s {
            brake(this.rover);
            this.phase = "done";
            // Means, computed with plain statements rather than inline `if`
            // EXPRESSIONS: rhai caps expression complexity and
            // `if n > 0 { sum / n.to_float() } else { 0.0 }` as a right-hand
            // side trips it at compile time.
            let sl_f = 0.0; let sr_f = 0.0;
            if this.n_fwd > 0 {
                let n = this.n_fwd.to_float();
                sl_f = this.spin_l_fwd / n;
                sr_f = this.spin_r_fwd / n;
            }
            let sl_r = 0.0; let sr_r = 0.0;
            if this.n_rev > 0 {
                let n = this.n_rev.to_float();
                sl_r = this.spin_l_rev / n;
                sr_r = this.spin_r_rev / n;
            }
            let sl_p = 0.0; let sr_p = 0.0;
            if this.n_piv > 0 {
                let n = this.n_piv.to_float();
                sl_p = this.spin_l_piv / n;
                sr_p = this.spin_r_piv / n;
            }
            // An explicit map, NOT `this`: helpers never see `this` (the runtime
            // binds it only in the hook it calls directly), and passing a plain
            // map keeps the verdict a pure function of the measurements.
            verdict(#{
                kernel: get(this.rover, "DriveMix.kernel"),
                ports:  get(this.rover, "DriveMix.ports"),
                d_fwd: this.d_fwd, along_fwd: this.along_fwd,
                d_rev: this.d_rev, along_rev: this.along_rev,
                sl_f: sl_f, sr_f: sr_f,
                sl_r: sl_r, sr_r: sr_r,
                sl_p: sl_p, sr_p: sr_p,
                yaw_pivot: this.yaw_pivot, d_pivot: this.d_pivot,
                yaw_prio: this.yaw_prio
            });
        }
        return;
    }
}

/// Stop the rover if the scenario is detached or hot-reloaded mid-run.
fn on_stop(me) {
    let r = find("/DrivetrainParity/RoverRaycast");
    if r >= 0 { brake(r); }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// Each block below names the unit test it stands in for. Bounds are set far
// from the defect they guard, not close to the expected value: this is a
// regression net for a refactor, and it must not fire because someone
// legitimately retuned a suspension.
fn verdict(s) {
    let f = [];

    // ── (3) `skid_kernel_maps_to_named_ports` — exact ──────────────────────
    // The unit test asserted `out[0].0 == "drive_left"` and
    // `out[1].0 == "drive_right"`. `DriveMix.ports` is that same ordered list,
    // reflected out of the live ECS: [left, right], positionally, which is what
    // the `skid` kernel's `ports.first()` / `ports.get(1)` consume.
    f.push(t_true(s.kernel == "skid",
                  "drive kernel is '" + s.kernel + "', expected 'skid'"));

    if type_of(s.ports) != "array" {
        f.push("DriveMix.ports is not an array - the skid mix never named its ports");
    } else {
        let m_n = "DriveMix.ports has " + s.ports.len() + " entries, expected 2 [left, right]";
        f.push(t_true(s.ports.len() == 2, m_n));
        if s.ports.len() == 2 {
            let m_l = "DriveMix.ports[0] is '" + s.ports[0] + "', expected 'drive_left'";
            f.push(t_true(s.ports[0] == "drive_left", m_l));
            let m_r = "DriveMix.ports[1] is '" + s.ports[1] + "', expected 'drive_right'";
            f.push(t_true(s.ports[1] == "drive_right", m_r));
        }
    }

    // ── (5) `registry_defaults_resolve_by_name` — positive half only ───────
    // The name resolved to a WORKING kernel: an unresolved name leaves the
    // vessel un-actuated (`apply_drive_mix` fail-safe coast), so real travel is
    // itself the proof. The negative half (`get("nope").is_none()`) has no
    // scenario-level equivalent — see the header.
    f.push(t_min(s.d_fwd, 15.0,
                 "forward-leg distance (m) - kernel 'skid' resolved but the rover never moved"));

    // ── (2) `skid_reverse_mirrors_forward_at_zero_steer` ───────────────────
    // The unit test asserted `lf + lr == 0` and `rf + rr == 0`: pure throttle
    // mirrors cleanly at zero steer. Observably that is three things — the rover
    // travels the same distance, it travels the OTHER way along the same
    // heading, and both wheel banks reverse their spin.
    f.push(t_rel(s.d_fwd, s.d_rev, 25.0, "reverse mirrors forward: distance (m)"));

    let m_dir = "reverse did not mirror forward: along-heading travel was ";
    m_dir += r2(s.along_fwd) + "m forward and " + r2(s.along_rev);
    m_dir += "m reverse - the two legs must have OPPOSITE signs along the same heading";
    f.push(t_true(!same_sign(s.along_fwd, s.along_rev), m_dir));

    let m_spl = "left bank did not reverse: spin was " + r2(s.sl_f);
    m_spl += " rad/s forward and " + r2(s.sl_r) + " rad/s in reverse";
    f.push(t_true(!same_sign(s.sl_f, s.sl_r), m_spl));

    let m_spr = "right bank did not reverse: spin was " + r2(s.sr_f);
    m_spr += " rad/s forward and " + r2(s.sr_r) + " rad/s in reverse";
    f.push(t_true(!same_sign(s.sr_f, s.sr_r), m_spr));

    // At zero steer the two banks must agree with EACH OTHER, in both
    // directions — that is the "pure throttle drives both sides equally" half of
    // the mirror, and it is what a leaked steering term in the drive allocation
    // would break first.
    f.push(t_rel(s.sl_f, s.sr_f, 20.0, "forward: left vs right bank spin (rad/s)"));
    f.push(t_rel(s.sl_r, s.sr_r, 20.0, "reverse: left vs right bank spin (rad/s)"));

    // ── (1a) inner-side COUNTER-ROTATION, directly observed at zero throttle ─
    // `skid_mix_norm(0, 1)` is `(+1, -1)`: at zero throttle the mix is
    // unambiguous, the ground cannot drag a wheel the wrong way because the rover
    // is not travelling, and the counter-rotation the unit test named is visible
    // as two banks spinning opposite ways. The 0.5 rad/s dead band is well below
    // the several rad/s a pivoting bank actually reaches and well above sensor
    // creep.
    f.push(t_opposite(s.sl_p, s.sr_p, 0.5,
                      "zero-throttle full-steer: bank counter-rotation (rad/s)"));

    // The left bank must spin the way it spun when driving FORWARD (the mix gives
    // it +1), which pins the counter-rotation to the correct side rather than
    // merely to "some side". Derived from this rover's own forward leg, so no
    // global sign convention is assumed.
    let m_side = "the wrong bank counter-rotated: at steer +1 the LEFT bank carries +1";
    m_side += " and must spin the same way it did driving forward (fwd L=" + r2(s.sl_f);
    m_side += ", pivot L=" + r2(s.sl_p) + ")";
    f.push(t_true(same_sign(s.sl_f, s.sl_p), m_side));

    // A pivot is a rotation, not a drive.
    f.push(t_min(s.yaw_pivot, 60.0, "zero-throttle full-steer: swept yaw (deg)"));
    f.push(t_max(s.d_pivot, 4.0, "zero-throttle full-steer: travel (m) - a pivot is not a drive"));

    // ── (1b) steer PRIORITY, as the property it exists to deliver ──────────
    // The exact `r = -1/3` under throttle is NOT observable — a wheel commanded
    // to -1/3 while the rover travels at 4.8 m/s is dragged forward by the ground
    // and its spin reports the ground, not the allocation. What steer priority
    // buys is stated in the kernel's own comment: without it "the outer side
    // saturates and steering becomes a lazy arc". So the assertion is yaw
    // AUTHORITY at full throttle. 45 deg over the priority leg is far below what
    // a steer-priority mix sweeps and far above a saturated lazy arc.
    let m_prio = "full-throttle full-steer swept only " + r2(s.yaw_prio);
    m_prio += "deg - steer priority (drive = forward * (1 - 0.5*|steer|)) exists so a";
    m_prio += " skid rover can turn sharply WHILE driving. A mix that clamps the inner";
    m_prio += " side to zero instead of letting it counter-rotate still turns, but lazily.";
    f.push(t_true(s.yaw_prio.abs() >= 45.0, m_prio));

    let m_prio_sign = "full-throttle steer +1 swept " + r2(s.yaw_prio);
    m_prio_sign += "deg but the zero-throttle pivot swept " + r2(s.yaw_pivot);
    m_prio_sign += "deg - the same steer command must turn the same way at any throttle";
    f.push(t_true(same_sign(s.yaw_prio, s.yaw_pivot), m_prio_sign));

    print("[spec] ── results ───────────────────────────────────────────────");
    print("[spec]   kernel                    " + s.kernel);
    // Printed element by element rather than as `+ s.ports`: rhai's `+` on a
    // string and an ARRAY is array concatenation semantics, not a debug format,
    // so the whole-array form is a compile-time type surprise waiting to happen.
    if type_of(s.ports) == "array" {
        let pl = "[spec]   ports                    ";
        for p in s.ports { pl += " " + p; }
        print(pl);
    }
    print("[spec]   forward dist / along (m)  " + r2(s.d_fwd) + " / " + r2(s.along_fwd));
    print("[spec]   reverse dist / along (m)  " + r2(s.d_rev) + " / " + r2(s.along_rev));
    print("[spec]   spin fwd  L / R (rad/s)   " + r2(s.sl_f) + " / " + r2(s.sr_f));
    print("[spec]   spin rev  L / R (rad/s)   " + r2(s.sl_r) + " / " + r2(s.sr_r));
    print("[spec]   spin piv  L / R (rad/s)   " + r2(s.sl_p) + " / " + r2(s.sr_p));
    print("[spec]   pivot yaw / travel        " + r2(s.yaw_pivot) + "deg / " + r2(s.d_pivot) + "m");
    print("[spec]   full-throttle steer yaw   " + r2(s.yaw_prio) + "deg");
    print("[spec]   NOT covered here: linear-mix projection (see six_independent_parity.rhai)");
    print("[spec]   NOT covered anywhere: registry lookup MISS for an unknown kernel name");

    let ok = t_report(f);
    emit("ALLOCATION_SPEC", if ok { "PASS" } else { "FAIL" });
    if ok {
        notify_kind("Allocation spec: PASS", "success");
    } else {
        notify_kind("Allocation spec: FAIL - drive allocation changed meaning", "error");
    }
}
