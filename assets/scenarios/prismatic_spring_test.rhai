// ── Raked prismatic springs under known loads ───────────────────────────────
//
// TEST-ONLY. Runs against `scenes/sandbox/prismatic_spring_test.usda`.
//
// WHAT IT GUARDS. That a `PhysicsPrismaticJoint` with a `PhysicsDriveAPI:linear`
// spring strokes under load when its axis is raked by `localRot0` / `localRot1`
// rather than being cardinal — and that it strokes with the RIGHT SIGN. That is
// the exact construction every landing leg uses, and a strut welded solid looks
// identical to a suspension in a screenshot.
//
// WHY IT IS NOT PART OF `landing_legs_test`. A leg lives in a mechanism: four
// raked sliders, four gimballed pads, four contacts and a hull. When such a gear
// does not stroke, "the joint is broken" and "the mechanism is over-constrained"
// look the same from outside. Here each joint carries one mass, with no contact
// and no second load path, so a failure has exactly one meaning.
//
// THE EXPECTED NUMBERS ARE ARITHMETIC, not recorded from a run:
//
//   axial load    m·g·cos(25°) = 500 · 1.62 · cos 25° = 734.1 N
//   travel        F / k        = 734.1 / 4000         = 0.1835 m
//
// SIGN. Displacement is measured along the joint frame's axis, which points
// body0 → body1:
//
//   EXTENSION    load pulls the mass further along the axis   +0.1835 m
//   COMPRESSION  load pushes it back toward body0             -0.1835 m
//
// A landing leg is the compression case — its axis points hull → foot and the
// ground pushes the foot back toward the hull — which is why a leg authors
// `lowerLimit = -0.8, upperLimit = 0.0`. If compression were the POSITIVE
// direction, that range would allow no travel at all: the leg would jam against
// its upper limit at zero stroke, carry the vehicle on a rigid constraint, and
// report no spring force. Both regimes are measured here so that question has an
// answer in the repository rather than in someone's head.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("PRISMATIC_SPRING", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// reads for its exit code.
//
// Assertion + verdict helpers come from the prelude (`prelude/auto_tests.rhai`).

const EXPECTED_TRAVEL = 0.1835;   // metres, magnitude
const EXPECTED_FORCE = 734.1;     // newtons along the strut axis, magnitude

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 5.0;    // free ring-down of a 4000 N/m, 2200 N·s/m strut
    this.sample_s = 0.5;
    this.next_sample = 0.0;
    this.phase = "settle";

    this.ext = find("/PrismaticSpringTest/StrutExt");
    this.comp = find("/PrismaticSpringTest/StrutComp");
    this.bob_ext = find("/PrismaticSpringTest/BobExt");
    this.bob_comp = find("/PrismaticSpringTest/BobComp");
    this.leglike = find("/PrismaticSpringTest/StrutLegLike");
    this.bob_leglike = find("/PrismaticSpringTest/BobLegLike");
    this.leglike_x0 = ();

    print("[strut] ── raked prismatic springs, isolated ─────────────────");
    print("[strut] extension joint gid=" + this.ext + "  compression gid=" + this.comp);
    print("[strut] expecting +" + EXPECTED_TRAVEL + " m / -" + EXPECTED_TRAVEL +
          " m at " + EXPECTED_FORCE + " N");
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    if this.ext == () || this.ext < 0 || this.comp == () || this.comp < 0 {
        fail_fast("strut prims not found (ext=" + this.ext + " comp=" + this.comp +
                  ") — wrong scene?", "PRISMATIC SPRING", "PRISMATIC_SPRING");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let xe = get(this.ext, "displacement");
    let fe = get(this.ext, "force");
    let xc = get(this.comp, "displacement");
    let fc = get(this.comp, "force");
    let xl = get(this.leglike, "displacement");
    let pl = world_pos(this.bob_leglike);
    if this.leglike_x0 == () && pl != () { this.leglike_x0 = pl[0]; }

    if this.t >= this.next_sample {
        print("[strut] t=" + r2(this.t) + "s  ext=" + r4(xe) + "m/" + r2(fe) +
              "N   comp=" + r4(xc) + "m/" + r2(fc) + "N   leglike=" + r4(xl) + "m");
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        verdict(#{
            ext_stroke: xe, ext_force: fe,
            comp_stroke: xc, comp_force: fc,
            ext_pos: world_pos(this.bob_ext),
            comp_pos: world_pos(this.bob_comp),
            leglike_stroke: xl,
            leglike_dx: if pl == () || this.leglike_x0 == () { () } else { pl[0] - this.leglike_x0 },
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
//   ports present    a joint publishing neither `displacement` nor `force` is not
//                    measurable, and a missing port read as 0.0 looks exactly
//                    like a rigid strut.
//   it strokes       the headline. A spring that does not move under 734 N is not
//                    a spring, whatever its authored stiffness claims.
//   by how much      within 20% of `F/k` — loose enough for solver damping and
//                    the strut's own settling, tight enough that a joint moving
//                    for some unrelated reason cannot pass.
//   the right way    extension POSITIVE, compression NEGATIVE. This is the check
//                    a landing leg's authored travel range depends on.
//   masses held      each bob hangs on its strut; reaching the ground 8 m below
//                    means the joint failed open rather than stiff.
fn verdict(s) {
    let f = [];
    f.push(t_present(s.ext_stroke, "extension strut displacement port"));
    f.push(t_present(s.ext_force, "extension strut force port"));
    f.push(t_present(s.comp_stroke, "compression strut displacement port"));
    f.push(t_present(s.comp_force, "compression strut force port"));

    if s.ext_stroke != () {
        f.push(t_true(s.ext_stroke.abs() > 0.001,
            "the extension strut does not stroke at all (" + r4(s.ext_stroke) +
            " m under " + EXPECTED_FORCE + " N) — the joint is rigid, not sprung"));
        f.push(t_true(s.ext_stroke > 0.0,
            "a strut being EXTENDED must read positive displacement, got " +
            r4(s.ext_stroke) + " m"));
        f.push(t_rel(s.ext_stroke, EXPECTED_TRAVEL, 0.2, "extension travel (m)"));
    }
    if s.comp_stroke != () {
        f.push(t_true(s.comp_stroke.abs() > 0.001,
            "the compression strut does not stroke at all (" + r4(s.comp_stroke) +
            " m under " + EXPECTED_FORCE + " N) — the joint is rigid, not sprung"));
        f.push(t_true(s.comp_stroke < 0.0,
            "a strut being COMPRESSED must read negative displacement, got " +
            r4(s.comp_stroke) + " m — a landing leg's `lowerLimit = -0.8, " +
            "upperLimit = 0.0` allows no travel if this sign is wrong"));
        f.push(t_rel(s.comp_stroke, -EXPECTED_TRAVEL, 0.2, "compression travel (m)"));
    }
    if s.ext_force != () {
        f.push(t_rel(s.ext_force.abs(), EXPECTED_FORCE, 0.2, "extension reaction (N)"));
    }
    if s.comp_force != () {
        f.push(t_rel(s.comp_force.abs(), EXPECTED_FORCE, 0.2, "compression reaction (N)"));
    }

    f.push(t_bounded(s.ext_pos, 100.0, "extension bob"));
    f.push(t_bounded(s.comp_pos, 100.0, "compression bob"));
    if s.ext_pos != () { f.push(t_range(s.ext_pos[1], 8.0, 9.5, "extension bob height (m)")); }
    if s.comp_pos != () { f.push(t_range(s.comp_pos[1], 8.0, 9.5, "compression bob height (m)")); }

    // The leg convention, measured on its own. A raked axis and a vertical one
    // differ by only 10% in travel — but by EVERYTHING in horizontal motion, so
    // the sideways displacement is the check that actually distinguishes them.
    if s.leglike_stroke != () {
        f.push(t_rel(s.leglike_stroke, EXPECTED_TRAVEL, 0.2,
            "leg-convention travel (m) — differing localRot0/localRot1"));
    }
    if s.leglike_dx != () {
        f.push(t_rel(s.leglike_dx, 0.0776, 0.25,
            "leg-convention sideways travel (m) — 0 would mean the 25° rake was " +
            "lost and the strut slides straight down"));
    }

    print("[strut] ── results ───────────────────────────────────────────");
    print("[strut]   leg-like     " + r4(s.leglike_stroke) + " m   dx " + r4(s.leglike_dx) + " m");
    print("[strut]   extension    " + r4(s.ext_stroke) + " m  " + r2(s.ext_force) + " N");
    print("[strut]   compression  " + r4(s.comp_stroke) + " m  " + r2(s.comp_force) + " N");
    print("[strut]   expected     +" + EXPECTED_TRAVEL + " / -" + EXPECTED_TRAVEL +
          " m at " + EXPECTED_FORCE + " N");

    report_verdict(f, "PRISMATIC SPRING", "PRISMATIC_SPRING");
}
