// ── Drive-allocation SPEC — the scenario-level replacement for the kernel unit tests ──
//
// TEST-ONLY. Runs against `scenes/sandbox/allocation_spec.usda`, which that
// scene attaches itself — loading the scene runs the spec:
//
//     sandbox --scene scenes/sandbox/allocation_spec.usda
//
// That rig carries TWO skid rovers differing in exactly one authored opinion,
// the name of their drive kernel: `RoverSkid` (stock, kernel `"skid"`, on which
// every allocation measurement is taken) and `RoverBogus` (an unresolvable
// `lunco:driveKernel`, which exists solely to prove the lookup MISS).
//
// ── WHY THIS FILE EXISTS ────────────────────────────────────────────────────
//
// `crates/lunco-mobility/src/kernels.rs` carries five unit tests that are the
// ONLY executable statement of what rover drive allocation MEANS:
//
//   1. `skid_steer_priority_lets_the_inner_side_counter_rotate`   (PART kept in Rust)
//   2. `skid_reverse_mirrors_forward_at_zero_steer`                (replaced, deleted)
//   3. `skid_kernel_maps_to_named_ports`                           (replaced, deleted)
//   4. `linear_kernel_projects_each_entry`                         (replaced, deleted)
//   5. `registry_defaults_resolve_by_name`                         (replaced, deleted)
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
//       Because the exact -1/3 survives nowhere else, the Rust unit test
//       `skid_steer_priority_lets_the_inner_side_counter_rotate` is deliberately
//       KEPT rather than deleted with its four siblings. Deleting an unreplaced
//       spec is worse than the doctrine violation of leaving one in Rust.
//
//   (2) reverse mirrors forward at zero steer — COVERED, as motion. Equal
//       distance, opposite direction, and both banks' spin sign flipped.
//
//   (3) skid maps to NAMED ports — COVERED exactly. `DriveMix.ports` is
//       reflected out of the live ECS and compared against
//       `["drive_left", "drive_right"]`.
//
//   (4) the linear mix projects each entry — COVERED, but ELSEWHERE, because
//       both of this scene's rovers are SKID rovers and have no linear mix to
//       project. Two scenarios carry it between them, and together they cover
//       the unit test's mix exactly:
//         * `six_independent_parity_test.rhai` reads the six-channel
//           `DriveMix.entries` back out of the engine, asserts each channel's
//           `forward`/`steer` coefficients, and then pins the PROJECTION through
//           motion — straight drive (the `forward` term), each steer direction
//           (the signed `steer` term), and a zero-throttle pivot that can only
//           happen if the right bank's `steer = -1` is really applied.
//         * `ackermann_parity_test.rhai` covers the unit test's own mix,
//           `drive_left=1,0 drive_right=1,0 steering=0,1`, including that the
//           drive ports carry NO steering term at steer 0 and that the
//           dedicated `steering` entry reaches a real actuator.
//       What neither pins is the `clamp(-1, 1)` on an entry whose terms sum past
//       ±1; no authored mix in the repo over-drives a channel. That is a
//       genuine, small loss, recorded here rather than papered over.
//
//   (5) the registry resolves kernels by name — COVERED, both halves.
//       POSITIVE: `"skid"` resolves to a WORKING kernel, asserted twice over —
//       `DriveMix.kernel` reads back as `"skid"`, and `RoverSkid` then actually
//       drives, which it could not do through an unresolved name.
//       NEGATIVE (`reg.get("nope").is_none()`): `RoverBogus` in this scene is
//       authored with `lunco:driveKernel = "no_such_kernel__failsafe_probe"`, a
//       name that is in neither the registry nor `lunco_hooks`. The FAILSAFE leg
//       below holds it at FULL THROTTLE for the entire run and asserts it does
//       not move, while `RoverSkid` — same asset, same ground, same friction,
//       same throttle — drives away. That is a stronger statement than the unit
//       test made: `is_none()` only says the lookup missed, whereas this says
//       what the miss MEANS, that `apply_drive_mix` coasts rather than falling
//       back to some other kernel.
//       This is not a hypothetical. `components/mobility/drive_laws/*.usda`
//       author `lunco:driveKernel = "external"` — a deliberately unresolvable
//       name — to stand the built-in mixing down so a Modelica drive law owns
//       the drive ports alone. If an unknown kernel ever silently fell back to
//       skid, every Modelica-driven rover would acquire a second writer on its
//       ports. This leg is the regression net under that mechanism.
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

/// A magnitude must EXCEED a bound — the anti-dead-test guard.
fn t_min(v, lo, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v.abs() >= lo { return ""; }
    return what + ": " + r2(v) + " below " + lo + " - nothing happened, so this is a dead test";
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

/// Signed heading change in DEGREES between two forward vectors, positive to the
/// RIGHT. Sign comes from the yaw-plane cross product.

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

    this.rover = find("/AllocationSpec/RoverSkid");
    // The skid rover's four wheels. FL/RL are the LEFT bank (drive_left), FR/RR
    // the RIGHT bank (drive_right) — the two ports the `skid` kernel writes.
    this.w_fl = find("/AllocationSpec/RoverSkid/Wheel_FL");
    this.w_fr = find("/AllocationSpec/RoverSkid/Wheel_FR");
    this.w_rl = find("/AllocationSpec/RoverSkid/Wheel_RL");
    this.w_rr = find("/AllocationSpec/RoverSkid/Wheel_RR");

    // The fail-safe control: same asset, unresolvable `lunco:driveKernel`.
    this.bogus = find("/AllocationSpec/RoverBogus");
    this.bogus_pos_prev = ();
    this.d_bogus = 0.0;      // travel accumulated while commanded to full throttle
    this.bogus_driven_s = 0.0;

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
    print("[spec] rover gid=" + this.rover + "  failsafe-control gid=" + this.bogus);
    print("[spec] wheels FL=" + this.w_fl + " FR=" + this.w_fr +
          " RL=" + this.w_rl + " RR=" + this.w_rr);
    print("[spec] legs: settle -> forward -> reverse -> zero-throttle pivot -> full-throttle steer");
    print("[spec] RoverBogus runs full throttle THROUGHOUT on an unresolvable kernel and must NOT move.");
    print("[spec] NOTE: `linear_kernel_projects_each_entry` is covered by");
    print("[spec]   six_independent_parity_test.rhai + ackermann_parity_test.rhai - both rovers here are SKID rovers.");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing rover is the classic silent no-op: `find` yields -1 and every
    // subsequent read is (). Fail on the FIRST tick instead of ticking forever.
    if this.rover == () || this.rover < 0 {
        print("  SPEC FAIL: rover prim not found (gid=" + this.rover +
              ") - wrong scene? expected scenes/sandbox/allocation_spec.usda");
        print("TESTS_FAIL 1/1");
        print("ALLOCATION SPEC: FAIL");
        emit("ALLOCATION_SPEC", "FAIL");
        this.phase = "done";
        return;
    }

    // The fail-safe control is not optional scenery: without it the negative
    // half of the registry test silently vanishes and the run still says PASS,
    // which is precisely the silent-scenario failure this file exists to avoid.
    if this.bogus == () || this.bogus < 0 {
        print("  SPEC FAIL: failsafe-control prim not found (gid=" + this.bogus +
              ") - expected /AllocationSpec/RoverBogus in scenes/sandbox/allocation_spec.usda");
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

    let dy = yaw_delta_deg(this.fprev, fwd);
    let step = seg(this.pos_prev, pos);
    this.pos_prev = pos;
    this.fprev = fwd;

    let spin_l = bank_spin(this.w_fl, this.w_rl);
    let spin_r = bank_spin(this.w_fr, this.w_rr);

    let since = t - this.phase_start;

    // ── The fail-safe control, commanded on EVERY leg ───────────────────────
    // `RoverBogus` is held at FULL FORWARD THROTTLE from the end of the settle
    // window to the end of the run — through legs where the measured rover is
    // reversing, pivoting and braking, because its own command never changes and
    // there is nothing to synchronise it with. The command is dispatched
    // identically to the measured rover's; the only difference between the two
    // vessels is that this one's `DriveMix.kernel` names nothing, so
    // `apply_drive_mix` writes no port and the vessel coasts.
    //
    // Travel is accumulated as PATH LENGTH, not displacement: a rover that
    // drove a wide circle back to its start has still moved, and a displacement
    // measure would call that stationary.
    let bpos = world_pos(this.bogus);
    if this.phase == "settle" {
        brake(this.bogus);
    } else {
        drive(this.bogus, 1.0, 0.0);
        this.d_bogus += seg(this.bogus_pos_prev, bpos);
        this.bogus_driven_s += dt();
    }
    this.bogus_pos_prev = bpos;

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
                yaw_prio: this.yaw_prio,
                bogus_kernel: get(this.bogus, "DriveMix.kernel"),
                bogus_ports:  get(this.bogus, "DriveMix.ports"),
                bogus_entries: get(this.bogus, "DriveMix.entries"),
                d_bogus: this.d_bogus,
                bogus_driven_s: this.bogus_driven_s
            });
        }
        return;
    }
}

/// Stop the rover if the scenario is detached or hot-reloaded mid-run.
fn on_stop(me) {
    let r = find("/AllocationSpec/RoverSkid");
    if r >= 0 { brake(r); }
    // The control rover is left commanded to full throttle by the run; if the
    // scenario is detached mid-leg and the kernel name is ever made resolvable,
    // an un-braked vessel would drive off on its own.
    let b = find("/AllocationSpec/RoverBogus");
    if b >= 0 { brake(b); }
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

    // ── (5a) `registry_defaults_resolve_by_name` — the POSITIVE half ───────
    // The name resolved to a WORKING kernel: an unresolved name leaves the
    // vessel un-actuated (`apply_drive_mix` fail-safe coast), so real travel is
    // itself the proof. This assertion is also the anchor of the negative half
    // below — the two rovers are the same asset on the same ground under the
    // same throttle, so "one drove and one did not" only means something while
    // this one demonstrably drove.
    f.push(t_min(s.d_fwd, 15.0,
                 "forward-leg distance (m) - kernel 'skid' resolved but the rover never moved"));

    // ── (5b) `registry_defaults_resolve_by_name` — the NEGATIVE half ───────
    // `assert!(reg.get("nope").is_none())`, restated as the consequence the
    // engine relies on: a kernel name that resolves to nothing must leave the
    // vessel UN-ACTUATED, not fall back to some default mix.
    //
    // First, that the vessel really is in the un-resolvable state — a scene edit
    // that dropped the attribute, or a reader that stopped honouring it, would
    // otherwise turn this leg into a second (passing) skid rover, and "it didn't
    // move" would be the only thing left to notice, at which point it wouldn't.
    let m_bk = "failsafe control's DriveMix.kernel is '" + s.bogus_kernel;
    m_bk += "', expected 'no_such_kernel__failsafe_probe' - the authored";
    m_bk += " lunco:driveKernel did not reach the vessel, so this leg proves nothing";
    f.push(t_true(s.bogus_kernel == "no_such_kernel__failsafe_probe", m_bk));

    // A scripted/unknown kernel carries no ports and no entries: `derive_drive_mix`
    // takes the `lunco:driveKernel` branch and never reaches the tank-differential
    // one. A non-empty list here means a built-in mix was ALSO derived.
    if type_of(s.bogus_ports) == "array" {
        let m_bp = "failsafe control has " + s.bogus_ports.len();
        m_bp += " DriveMix.ports, expected 0 - an unresolvable kernel must not also";
        m_bp += " derive the built-in skid port pair";
        f.push(t_true(s.bogus_ports.len() == 0, m_bp));
    }
    if type_of(s.bogus_entries) == "array" {
        let m_be = "failsafe control has " + s.bogus_entries.len();
        m_be += " DriveMix.entries, expected 0 - an unresolvable kernel must not also";
        m_be += " derive a linear mix table";
        f.push(t_true(s.bogus_entries.len() == 0, m_be));
    }

    // Second, that the command was actually ISSUED for a long window. Without
    // this the "it did not move" assertion passes trivially on a run where the
    // legs never advanced and nothing was ever commanded at all.
    f.push(t_min(s.bogus_driven_s, 20.0,
                 "seconds the failsafe control was held at full throttle - too short to conclude anything"));

    // Third, the assertion itself. The bound is 3 m against a rover that covers
    // 15 m in the forward leg alone: far enough from settle-jitter to never fire
    // spuriously, and far enough below actuated travel that any fallback mix —
    // even a weak one — trips it.
    let m_fs = "FAIL-SAFE BROKEN: the vessel with kernel '" + s.bogus_kernel;
    m_fs += "' travelled " + r2(s.d_bogus) + "m while held at full throttle for ";
    m_fs += r2(s.bogus_driven_s) + "s. An unresolvable drive kernel must leave the";
    m_fs += " vessel un-actuated (apply_drive_mix coasts) - anything else means an";
    m_fs += " unknown name now falls back to a real mix, which would put a SECOND";
    m_fs += " writer on the drive ports of every Modelica drive law";
    m_fs += " (lunco:driveKernel = 'external').";
    f.push(t_true(s.d_bogus.abs() <= 3.0, m_fs));

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
    print("[spec]   failsafe kernel name      " + s.bogus_kernel);
    let bl = "[spec]   failsafe travel (m)       " + r2(s.d_bogus);
    bl += "  over " + r2(s.bogus_driven_s) + "s at full throttle";
    print(bl);
    print("[spec]   NOT covered here: linear-mix projection (see six_independent_parity_test.rhai");
    print("[spec]     and ackermann_parity_test.rhai - both rovers in this scene are SKID rovers)");
    print("[spec]   STILL IN RUST: the exact skid_mix_norm(1,1) = (1, -1/3) inner-side value;");
    print("[spec]     under throttle the ground drags the wheel, so -1/3 is not observable.");

    report_verdict(f, "ALLOCATION SPEC", "ALLOCATION_SPEC");
}
