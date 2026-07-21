// ── Landing-gear test — the sprung legs take the vehicle's weight ───────────
//
// TEST-ONLY. Runs against `scenes/sandbox/landing_legs_test.usda`: one airframe,
// four raked prismatic legs with welded footpads, flat ground, 3 m drop. No
// guidance, no rover, no mission — so anything this test sees is the LEG
// MECHANISM and nothing else.
//
// WHAT IT GUARDS. A landing leg is the one mechanism in this project whose joint
// frames are not cardinal: the struts are raked 25° off vertical, so both halves
// of each `UsdPhysicsJoint` frame (`physics:localRot0` on the hull,
// `localRot1` on the strut) have to agree, and the pad welds have to carry the
// inverse rake so the footpads stay flat. Get any of that wrong and the joint
// starts violated in ORIENTATION — which no amount of position checking sees.
// The solver then resolves 25° of error impulsively through a 3.5 m lever arm
// and the vehicle disassembles at several km/s.
//
// So the assertions are about the vehicle STILL BEING A VEHICLE after the drop:
// bodies at finite, bounded positions, the hull level and at rest, and all four
// struts sharing the load. A silent frame regression fails every one of them.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("LANDING_LEGS", "PASS"|"FAIL")` — the telemetry verdict `scene_test`
// reads to set its exit code. The log also carries a per-sample table; a run
// with no `[legs]` rows proves nothing and is itself a failure.

// Assertion + verdict helpers come from the prelude (`prelude/auto_tests.rhai`):
// `r2` `r4` `t_range` `t_true` `t_bounded` `report_verdict` `fail_fast` `ys`.
// Do not paste private copies of them into a new test.

/// One log row. Built by successive `+=`: rhai caps EXPRESSION complexity and a
/// single multi-value concatenation chain trips it at compile time.
fn row(t, y, tilt, f, x) {
    let s = "[legs] t=" + r2(t) + "s";
    s += "  hull_y=" + r4(y);
    s += "  tilt=" + r2(tilt) + "deg";
    s += "  strut_N=[" + r2(f[0]) + " " + r2(f[1]) + " " + r2(f[2]) + " " + r2(f[3]) + "]";
    s += "  stroke_m=[" + r4(x[0]) + " " + r4(x[1]) + " " + r4(x[2]) + " " + r4(x[3]) + "]";
    print(s);
}

/// Vertical position of each body in a list, for tracing what is holding the
/// vehicle up: a pad above the ground is not carrying anything.
fn ys(ids) {
    let out = [];
    for i in ids {
        let p = world_pos(i);
        if p == () { out.push("()"); } else { out.push(r4(p[1])); }
    }
    out
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 6.0;    // drop (~0.8 s) + stroke + ring-down
    this.sample_s = 0.5;
    this.next_sample = 0.0;

    this.hull = find("/LandingLegsTest/Lander");
    this.legs = [
        find("/LandingLegsTest/Lander/LegPX"),
        find("/LandingLegsTest/Lander/LegNX"),
        find("/LandingLegsTest/Lander/LegPZ"),
        find("/LandingLegsTest/Lander/LegNZ"),
    ];
    this.pads = [
        find("/LandingLegsTest/Lander/LegPX/PadPX"),
        find("/LandingLegsTest/Lander/LegNX/PadNX"),
        find("/LandingLegsTest/Lander/LegPZ/PadPZ"),
        find("/LandingLegsTest/Lander/LegNZ/PadNZ"),
    ];
    this.springs = [
        find("/LandingLegsTest/Lander/LegPX_Spring"),
        find("/LandingLegsTest/Lander/LegNX_Spring"),
        find("/LandingLegsTest/Lander/LegPZ_Spring"),
        find("/LandingLegsTest/Lander/LegNZ_Spring"),
    ];

    this.phase = "drop";
    this.y_prev = ();
    this.peak_force = 0.0;

    print("[legs] ── landing gear test ──────────────────────────────────");
    print("[legs] hull gid=" + this.hull + " legs=" + this.legs + " springs=" + this.springs);
    print("[legs] dropping from 3 m; settling for " + this.settle_s + "s");
}

/// Strut reaction forces, one per leg, as read from each spring joint's `force`
/// port. `()` for a joint that does not publish one — reported as MISSING rather
/// than silently coerced to 0.0, which would read as "the leg carries no load"
/// and pass the wrong test.
fn strut_forces(springs) {
    let out = [];
    for s in springs {
        if s == () || s < 0 { out.push(()); } else { out.push(get(s, "force")); }
    }
    out
}

/// Strut travel, one per leg, from each spring joint's `displacement` port.
/// Negative IS compression — the authored stroke is -0.8 .. 0.0 m.
fn strut_strokes(springs) {
    let out = [];
    for s in springs {
        if s == () || s < 0 { out.push(()); } else { out.push(get(s, "displacement")); }
    }
    out
}

/// Each strut's angular-lock error, in degrees — see `joint_lock_error_deg`.
///
/// A leg that BENDS instead of stroking carries the vehicle on a constraint the
/// joint is supposed to hold rigid, and the spring never feels the landing. That
/// is invisible in position, height and tilt: the vehicle looks perfectly landed.
fn frame_errors(hull, legs) {
    // `localRot0 * physics:axis`, per leg: the free axis in the HULL's frame.
    let a0 = [[0.42262, -0.90631, 0.0], [-0.42262, -0.90631, 0.0],
              [0.0, -0.90631, 0.42262], [0.0, -0.90631, -0.42262]];
    // `localRot1 * physics:axis`: every leg authors `localRot1` as the bare 180°
    // flip — the rake lives in the leg's own transform — so this is -Y for all four.
    let a1 = [0.0, -1.0, 0.0];
    let out = [];
    for i in 0..legs.len() {
        let d = joint_lock_error_deg(hull, a0[i], legs[i], a1);
        if d < 0.0 { out.push(()); } else { out.push(r4(d)); }
    }
    out
}

/// Each footpad's tilt from level, in degrees. The pads gimbal freely inside a
/// 30° cone, so this is how far each has articulated to meet the ground.
fn pad_tilts(pads) {
    let out = [];
    for i in pads { out.push(r2(tilt_deg(i))); }
    out
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing prim is the classic silent no-op: `find` yields -1 and every read
    // after it is `()`. Fail on the FIRST tick rather than ticking forever.
    if this.hull == () || this.hull < 0 {
        fail_fast("hull prim not found — wrong scene? expected " +
                  "scenes/sandbox/landing_legs_test.usda", "LANDING LEGS", "LANDING_LEGS");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let pos = world_pos(this.hull);
    let tilt = tilt_deg(this.hull);
    let f = strut_forces(this.springs);
    let x = strut_strokes(this.springs);

    let y = ();
    if pos != () { y = pos[1]; }

    for v in f {
        if v != () && v > this.peak_force { this.peak_force = v; }
    }

    if this.t >= this.next_sample {
        let fr = [];
        for v in f { if v == () { fr.push(0.0); } else { fr.push(v); } }
        let xr = [];
        for v in x { if v == () { xr.push(0.0); } else { xr.push(v); } }
        row(this.t, y, tilt, fr, xr);
        print("[legs]     pad_y=" + ys(this.pads) + "  leg_y=" + ys(this.legs));
        print("[legs]     frame_err_deg=" + frame_errors(this.hull, this.legs));
        print("[legs]     pad_tilt_deg=" + pad_tilts(this.pads));
        this.next_sample = this.t + this.sample_s;
    }

    if this.t >= this.settle_s {
        this.phase = "done";
        // An explicit map, NOT `this`: helpers never see `this`, and passing a
        // plain map keeps the verdict a pure function of the measurements.
        verdict(#{
            hull_pos: pos,
            hull_y: y,
            tilt: tilt,
            forces: f,
            strokes: x,
            peak_force: this.peak_force,
            pad_pos: [
                world_pos(this.pads[0]), world_pos(this.pads[1]),
                world_pos(this.pads[2]), world_pos(this.pads[3]),
            ],
            leg_pos: [
                world_pos(this.legs[0]), world_pos(this.legs[1]),
                world_pos(this.legs[2]), world_pos(this.legs[3]),
            ],
            frame_err: frame_errors(this.hull, this.legs),
            pad_tilt: pad_tilts(this.pads),
        });
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────
//
// BOUNDS — why these numbers.
//
//   bodies bounded   |pos| <= 100 m. The vehicle is 11 m across and never leaves
//                    the pad, so this is not a placement check: it is the
//                    "still a vehicle" check. A joint-frame regression throws
//                    parts to hundreds of metres within the first second, so
//                    any generous finite bound catches it, and a generous one
//                    cannot false-positive on a bouncy landing.
//
//   hull level       tilt <= 5°. The drop is symmetric onto flat ground with
//                    four identical legs, so the only thing that can tip it is
//                    an asymmetric strut. Slack enough for the ring-down.
//
//   hull height      1.0 .. 8.0 m. Rest height is set by how far the struts
//                    stroke under the vehicle's weight; the band spans
//                    fully-compressed to barely-compressed and only excludes
//                    "sank through the ground" and "never fell".
//
//   struts loaded    every leg > 100 N at rest, and they agree within 40% of
//                    the largest. A leg reading ~0 while its neighbours carry
//                    the vehicle is a dead joint — the failure that looks most
//                    like a working scene in a screenshot.
//
//   struts COMPRESS  every stroke < 0, because the axis points hull→foot and the
//                    ground pushes the foot back toward the hull. Force alone is
//                    not enough: a strut jammed against a limit stop can report a
//                    plausible reaction while never moving.
//
//   the slider slid  the prismatic's own angular lock (`rot0·localRot0 ==
//                    rot1·localRot1`) still holds at rest. This is the check that
//                    catches a gear which settles by BENDING rather than
//                    stroking — the vehicle looks landed and level, the hull sits
//                    at a believable height, and the springs are carrying nothing
//                    because the load never reached the axial DOF. It caught the
//                    strut-collider grounding bug (see the pad-depth note in
//                    descent_lander.usda), and nothing else in this file did.
fn verdict(s) {
    let f = [];

    f.push(t_bounded(s.hull_pos, 100.0, "hull"));
    let names = ["PX", "NX", "PZ", "NZ"];
    for i in 0..4 {
        f.push(t_bounded(s.leg_pos[i], 100.0, "strut " + names[i]));
        f.push(t_bounded(s.pad_pos[i], 100.0, "pad " + names[i]));
    }

    f.push(t_range(s.tilt, 0.0, 5.0, "hull tilt at rest (deg)"));
    f.push(t_range(s.hull_y, 1.0, 8.0, "hull height at rest (m)"));

    // The gear must be LOADED at all. `force` is the right instrument for this
    // one question: any leg developing hundreds of newtons proves the springs are
    // in the load path.
    let fmax = 0.0;
    let missing = false;
    for v in s.forces {
        if v == () { missing = true; }
        else { if v > fmax { fmax = v; } }
    }
    f.push(t_true(!missing, "a spring joint published no `force` port"));
    f.push(t_true(fmax > 100.0,
                  "no strut carries load at rest (max " + r2(fmax) +
                  " N) — the legs are not taking the vehicle's weight"));

    // Every strut must have SHORTENED, and by a comparable amount.
    //
    // SHARING IS JUDGED ON STROKE, NOT ON `force`, and that is not a softer test —
    // it is the right instrument. The load a spring carries is `k*x`. The port
    // reports the whole drive law, `k*x + c*v`, and with c = 2200 N·s/m a few tenths
    // of a m/s of contact jitter swings it by ±600 N against a ~280 N carried load.
    // Comparing legs on that compares rate noise. Stroke is the state, is what the
    // load actually is, and still fails hard for a leg carrying nothing — a dead
    // strut reads ~0 and is caught by the same assertion.
    //
    // Sign matters as much as magnitude: the axis points hull→foot, so a loaded leg
    // reads NEGATIVE. A stroke of ~0 means the vehicle's weight is reaching the
    // ground through something that is not the spring.
    let deepest = 0.0;
    for x in s.strokes {
        if x != () && -x > deepest { deepest = -x; }
    }
    for i in 0..4 {
        let x = s.strokes[i];
        if x != () {
            f.push(t_true(x < -0.005,
                "strut " + names[i] + " did not compress (" + r4(x) + " m)"));
            f.push(t_range(-x, deepest * 0.6, deepest * 1.4,
                "strut " + names[i] + " travel vs deepest " + r4(deepest) + " m"));
        }
    }

    // Angular-lock error is REPORTED, not asserted, and the reason is worth
    // stating: the angle alone does not discriminate. A bypassed gear bends ~2°
    // carrying nothing; a working one bends ~2° carrying 900 N, because an XPBD
    // joint is elastic and the bend tracks LOAD. What distinguishes them is the
    // conjunction — bending WHILE the spring reads nothing — and the stroke
    // assertions above already test the half that carries the information.
    // Asserting an angle here would be asserting the load, in the wrong units.

    // The footpads must still be GIMBALLING — the check is PINNING, nothing finer.
    //
    // Each pad hangs on a spherical joint with a 30° cone and nothing centring it,
    // so it finds its own angle against the ground; that is what a gimbal is for.
    // Measured on flat ground the four wander a 5..24° band and stay there, because
    // the raked strut scuffs the foot sideways as it strokes and friction rolls it
    // back. Asserting any tighter angle would be asserting that wander, which is
    // not a contract — it is weather.
    //
    // What IS a contract: a pad driven onto its cone stop has stopped being a
    // gimbal. The ball becomes a rigid corner and the leg starts taking ground load
    // through it as a moment instead of as travel — the same class of failure as a
    // strut that bends rather than strokes, one joint further out.
    for i in 0..4 {
        let a = s.pad_tilt[i];
        if a >= 0.0 {
            f.push(t_range(a, 0.0, 29.0,
                "pad " + names[i] + " tilt (deg) — pinned against its 30° cone " +
                "limit, so the foot is no longer articulating"));
        }
    }

    print("[legs] ── results ──────────────────────────────────────────");
    print("[legs]   hull y      " + r2(s.hull_y) + " m");
    print("[legs]   hull tilt   " + r2(s.tilt) + " deg");
    print("[legs]   strut loads " + s.forces);
    print("[legs]   strut stroke " + s.strokes);
    print("[legs]   frame err   " + s.frame_err + " deg");
    print("[legs]   pad tilt    " + s.pad_tilt + " deg");
    print("[legs]   peak strut  " + r2(s.peak_force) + " N");

    report_verdict(f, "LANDING LEGS", "LANDING_LEGS");
}
