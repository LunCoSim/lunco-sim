// ── Stack test — a payload rigidly bolted to a sprung vehicle ──────────────
//
// TEST-ONLY. Runs against `scenes/sandbox/lander_rover_stack_test.usda`.
//
// WHAT IT GUARDS. A `PhysicsFixedJoint` between two ARTICULATED vehicles merges
// their constraint graphs: the lander's four sprung legs and the rover's whole
// drivetrain become one island. Either vehicle can be perfectly well behaved
// alone and the pair still diverge, because a rigid joint is where mass ratios
// and redundant loops from both sides meet. This scene drops the pair and
// asserts the assembly is still an assembly afterwards.
//
// The check is deliberately about SURVIVAL and POSE, not about the suspension:
// `landing_legs_test` owns the strut behaviour, and duplicating it here would
// only make one bug fail two tests.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("LANDER_ROVER_STACK", "PASS"|"FAIL")` — the telemetry verdict
// `scene_test` reads for its exit code, plus a `[stack]` sample table.

fn r2(x) { (x * 100.0).round() / 100.0 }

/// Positive-form range test. Never write this as `!(v < lo || v > hi)`: NaN
/// compares false against everything, so the negated form admits exactly the
/// value this test exists to catch.
fn t_range(v, lo, hi, what) {
    if type_of(v) == "()" { return what + ": MISSING"; }
    if v >= lo && v <= hi { return ""; }
    return what + ": " + r2(v) + " outside [" + lo + ", " + hi + "]";
}

fn t_true(cond, what) { if cond { "" } else { what } }

/// A body must be finite and inside the scene. The "still an assembly" check.
fn t_bounded(p, what) {
    if type_of(p) == "()" { return what + ": MISSING (no world_pos)"; }
    let l = vlen(p);
    if l >= 0.0 && l <= 100.0 { return ""; }
    return what + ": |pos|=" + r2(l) + " m — left the scene (or is not finite)";
}

fn t_report(fails) {
    let real = [];
    for f in fails { if f != "" { real.push(f); } }
    if real.is_empty() {
        print("TESTS_OK " + fails.len());
        print("LANDER ROVER STACK: PASS");
        return true;
    }
    for f in real { print("  STACK FAIL: " + f); }
    print("TESTS_FAIL " + real.len() + "/" + fails.len());
    print("LANDER ROVER STACK: FAIL");
    false
}

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 8.0;
    this.sample_s = 1.0;
    this.next_sample = 0.0;
    this.phase = "drop";

    this.hull = find("/LanderRoverStack/Lander");
    this.rover = find("/LanderRoverStack/SkidRover");
    // The joint holds a fixed 1.65 m stack offset; drift in that gap is the
    // direct read on whether the weld is holding.
    this.gap0 = ();

    print("[stack] ── lander + rover stack test ────────────────────────");
    print("[stack] hull gid=" + this.hull + "  rover gid=" + this.rover);
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.hull == () || this.hull < 0 || this.rover == () || this.rover < 0 {
        print("  STACK FAIL: lander or rover prim not found (hull=" + this.hull +
              " rover=" + this.rover + ") — wrong scene?");
        print("TESTS_FAIL 1/1");
        print("LANDER ROVER STACK: FAIL");
        emit("LANDER_ROVER_STACK", "FAIL");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let ph = world_pos(this.hull);
    let pr = world_pos(this.rover);

    let gap = ();
    if type_of(ph) != "()" && type_of(pr) != "()" { gap = vlen(vsub(ph, pr)); }
    if this.gap0 == () && type_of(gap) != "()" { this.gap0 = gap; }

    if this.t >= this.next_sample {
        let hy = ();
        if type_of(ph) != "()" { hy = ph[1]; }
        let s = "[stack] t=" + r2(this.t) + "s";
        s += "  hull_y=" + r2(hy);
        s += "  tilt=" + r2(tilt_deg(this.hull)) + "deg";
        s += "  stack_gap=" + r2(gap) + "m";
        print(s);
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            hull_pos: ph,
            rover_pos: pr,
            hull_y: ph[1],
            tilt: tilt_deg(this.hull),
            rover_tilt: tilt_deg(this.rover),
            gap: gap,
            gap0: this.gap0,
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   bodies bounded   |pos| <= 100 m — the assembly is still an assembly.
//   stack gap        within 5 cm of where it started. A `PhysicsFixedJoint` is
//                    rigid, so any real drift means the weld is being pulled
//                    apart by the merged island rather than holding it together.
//   both level       <= 5 deg. Welded together and dropped flat, the pair share
//                    one attitude; a divergence shows up here long before the
//                    bodies actually escape.
//   hull height      1.0 .. 8.0 m — sank through the ground, or never fell.
fn verdict(s) {
    let f = [];
    f.push(t_bounded(s.hull_pos, "lander"));
    f.push(t_bounded(s.rover_pos, "rover"));
    f.push(t_range(s.tilt, 0.0, 5.0, "lander tilt at rest (deg)"));
    f.push(t_range(s.rover_tilt, 0.0, 5.0, "rover tilt at rest (deg)"));
    f.push(t_range(s.hull_y, 1.0, 8.0, "hull height at rest (m)"));

    if s.gap == () || s.gap0 == () {
        f.push("stack gap: MISSING");
    } else {
        let m = "stack gap drifted: " + r2(s.gap0) + " m -> " + r2(s.gap) +
                " m — the fixed joint is not holding the payload";
        f.push(t_true((s.gap - s.gap0).abs() < 0.05, m));
    }

    print("[stack] ── results ─────────────────────────────────────────");
    print("[stack]   hull y       " + r2(s.hull_y) + " m");
    print("[stack]   lander tilt  " + r2(s.tilt) + " deg");
    print("[stack]   rover tilt   " + r2(s.rover_tilt) + " deg");
    print("[stack]   stack gap    " + r2(s.gap0) + " -> " + r2(s.gap) + " m");

    let ok = t_report(f);
    emit("LANDER_ROVER_STACK", if ok { "PASS" } else { "FAIL" });
    if ok {
        notify_kind("Lander/rover stack: PASS", "success");
    } else {
        notify_kind("Lander/rover stack: FAIL — the assembly came apart", "error");
    }
}
