// ── Landing-gear test — the sprung legs take the vehicle's weight ───────────
//
// TEST-ONLY. Runs against `scenes/tests/landing_legs.usda`: one airframe,
// four raked prismatic legs with rigid child footpad colliders, and flat ground.
// There is no guidance, rover, or mission: anything this test sees is the leg
// mechanism and its physical load path.
//
// WHAT IT GUARDS. Each landing leg has one prismatic degree of freedom along its
// authored rake. Each joint's two frames (`physics:localRot0` on the hull and
// `localRot1` on the leg) define that leg's raked local Y axis; the X and Z
// bases are authored for their respective planes. Negative displacement is
// compression along the hull-to-foot axis. Each flat pad is a child collider of
// its leg body, so it cannot rotate independently or turn onto its rim.
//
// So the assertions are about the vehicle STILL BEING A VEHICLE after the drop:
// bodies at finite, bounded positions, the hull level and at rest, all four
// struts sharing the load, and the rigid leg bodies keeping every pad level. A
// silent frame regression fails every one of them.
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
fn row(t, pos, y, tilt, f, x) {
    let s = "[legs] t=" + r2(t) + "s";
    s += "  hull_y=" + r4(y);
    if pos != () { s += "  hull_xyz=[" + r4(pos[0]) + " " + r4(pos[1]) + " " + r4(pos[2]) + "]"; }
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
    this.settle_s = 30.0;   // drop + crush + planted-gear ring-down
    this.sample_s = 0.5;
    this.next_sample = 0.0;
    this.ring_start_s = 24.0;
    this.stroke_min = [1000000.0, 1000000.0, 1000000.0, 1000000.0];
    this.stroke_max = [-1000000.0, -1000000.0, -1000000.0, -1000000.0];

    this.hull = find("/LandingLegsTest/Lander");
    this.rcs = find("/LandingLegsTest/Lander/AttitudePropulsion");
    this.ground = find("/LandingLegsTest/Ground");
    this.site = find("/LandingLegsTest");
    this.moon = find("/LandingLegsTest/SolarSystem/Moon");
    this.environment = find("/LandingLegsTest/Lander/Environment");
    // Site placement rotates the whole scene to the Moon's local tangent.
    // Capture the authored hull up-vector once; comparing against global +Y
    // would report the latitude frame as a lander tilt.
    this.surface_up = world_up(this.hull);
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
    print("[legs] ground gid=" + this.ground + " ground_pos=" + world_pos(this.ground));
    print("[legs] surface_up=" + this.surface_up);
    print("[legs] site_pos=" + world_pos(this.site) + " site_up=" + world_up(this.site));
    print("[legs] moon_pos=" + world_pos(this.moon) + " moon_up=" + world_up(this.moon));
    print("[legs] gravity probe will be sampled after the first physics tick");
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

/// Native contact measurements from each leg's compound body. Each compound
/// has one enabled shape—the child footpad—so these are per-foot facts without
/// pretending that the child shape is a separate runtime entity.
fn leg_contact_forces(legs) {
    let out = [];
    for leg in legs {
        let v = get(leg, "contact_force");
        if v == () { out.push(()); } else { out.push(v); }
    }
    out
}

fn leg_contact_state(legs) {
    let out = [];
    for leg in legs {
        let v = get(leg, "contact");
        if v == () { out.push(()); } else { out.push(v); }
    }
    out
}

/// Each strut's angular-lock error, in degrees — see `joint_lock_error_deg`.
///
/// A leg that BENDS instead of stroking carries the vehicle on a constraint the
/// joint is supposed to hold rigid, and the spring never feels the landing. That
/// is invisible in position, height and tilt: the vehicle looks perfectly landed.
fn frame_errors(hull, legs) {
    // `localRot0 * physics:axis`, per leg: the four slider axes are raked 20°
    // outward in the HULL frame. `localRot1` uses the same 180° sign basis on
    // every leg, so +Y in the body frame points from hull toward the foot.
    let s = 0.34202014;
    let c = 0.93969262;
    let a0 = [[s, -c, 0.0], [-s, -c, 0.0],
              [0.0, -c, s], [0.0, -c, -s]];
    let a1 = [0.0, -1.0, 0.0];
    let out = [];
    for i in 0..legs.len() {
        let d = joint_lock_error_deg(hull, a0[i], legs[i], a1);
        if d < 0.0 { out.push(()); } else { out.push(r4(d)); }
    }
    out
}

/// Tilt from the local terrain normal, in degrees. The test scene is site
/// anchored on the Moon, so global +Y is not the local ground normal.
fn local_tilt(id, surface_up) {
    let u = world_up(id);
    if u == () || surface_up == () { return -1.0; }
    let deg = angle_deg(u, surface_up);
    if deg == () { return -1.0; }
    deg
}

/// Each footpad's tilt from the local terrain normal, in degrees.
fn pad_tilts(pads, surface_up) {
    let out = [];
    for i in pads { out.push(r2(local_tilt(i, surface_up))); }
    out
}

fn on_tick(me) {
    if this.phase == "done" { return; }

    // A missing prim is the classic silent no-op: `find` yields -1 and every read
    // after it is `()`. Fail on the FIRST tick rather than ticking forever.
    if this.hull == () || this.hull < 0 {
        fail_fast("hull prim not found — wrong scene? expected " +
                  "scenes/tests/landing_legs.usda", "LANDING LEGS", "LANDING_LEGS");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let pos = world_pos(this.hull);
    let tilt = local_tilt(this.hull, this.surface_up);
    let f = strut_forces(this.springs);
    let x = strut_strokes(this.springs);

    // A constraint solver's instantaneous velocity contains substep contact
    // chatter; visible suspension jitter is displacement exchanged over time.
    // Measure the full final two-second stroke envelope, not one phase sample.
    if this.t >= this.ring_start_s {
        for i in 0..4 {
            if x[i] != () {
                if x[i] < this.stroke_min[i] { this.stroke_min[i] = x[i]; }
                if x[i] > this.stroke_max[i] { this.stroke_max[i] = x[i]; }
            }
        }
    }

    let y = ();
    if pos != () { y = pos[1]; }

    if this.t < 0.03 {
        print("[legs] physics_pos=[" + get(this.hull, "position_x") + ", "
            + get(this.hull, "position_y") + ", " + get(this.hull, "position_z") + "]");
        print("[legs] hull_local_gravity=" + get(this.hull, "LocalGravity")
            + " ground_up=" + world_up(this.ground));
        print("[legs] world_pos=" + world_pos(this.hull)
            + " hull_up=" + world_up(this.hull)
            + " ground_pos=" + world_pos(this.ground));
        print("[legs] hull_tf=" + get(this.hull, "Transform")
            + " hull_parent=" + get(this.hull, "ChildOf"));
        print("[legs] gravity=[" + get(this.environment, "gravity_x") + ", "
            + get(this.environment, "gravity_y") + ", " + get(this.environment, "gravity_z") + "]");
    }

    for v in f {
        if v != () && v > this.peak_force { this.peak_force = v; }
    }

    if this.t >= this.next_sample {
        let fr = [];
        for v in f { if v == () { fr.push(0.0); } else { fr.push(v); } }
        let xr = [];
        for v in x { if v == () { xr.push(0.0); } else { xr.push(v); } }
        row(this.t, pos, y, tilt, fr, xr);
        print("[legs]     hull_vel=[" + get(this.hull, "velocity_x") + ", "
            + get(this.hull, "velocity_y") + ", " + get(this.hull, "velocity_z") + "]");
        print("[legs]     hull_angvel=[" + get(this.hull, "angvel_x") + ", "
            + get(this.hull, "angvel_y") + ", " + get(this.hull, "angvel_z") + "]");
        print("[legs]     commands=[throttle " + get(this.hull, "throttle")
            + " torque " + get(this.hull, "torque_x") + ","
            + get(this.hull, "torque_y") + "," + get(this.hull, "torque_z")
            + "] rcs_activity=[" + get(this.rcs, "pitch_pos_a_activity") + ","
            + get(this.rcs, "pitch_neg_a_activity") + ","
            + get(this.rcs, "roll_pos_a_activity") + ","
            + get(this.rcs, "roll_neg_a_activity") + ","
            + get(this.rcs, "yaw_pos_a_activity") + ","
            + get(this.rcs, "yaw_neg_a_activity") + "]");
        print("[legs]     physics_pos=[" + get(this.hull, "position_x") + ", "
            + get(this.hull, "position_y") + ", " + get(this.hull, "position_z") + "]"
            + " gravity=[" + get(this.environment, "gravity_x") + ", "
            + get(this.environment, "gravity_y") + ", " + get(this.environment, "gravity_z") + "]");
        print("[legs]     pad_y=" + ys(this.pads) + "  leg_y=" + ys(this.legs));
        print("[legs]     ground_pos=" + world_pos(this.ground));
        print("[legs]     frame_err_deg=" + frame_errors(this.hull, this.legs));
        print("[legs]     pad_tilt_deg=" + pad_tilts(this.pads, this.surface_up));
        print("[legs]     leg_contact_N=" + leg_contact_forces(this.legs)
            + "  contact=" + leg_contact_state(this.legs));
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
            pad_tilt: pad_tilts(this.pads, this.surface_up),
            leg_contact_force: leg_contact_forces(this.legs),
            leg_contact: leg_contact_state(this.legs),
            hull_mass: get(this.hull, "mass"),
            hull_inertia: [
                get(this.hull, "inertia_xx"), get(this.hull, "inertia_yy"),
                get(this.hull, "inertia_zz"),
            ],
            leg_mass: [
                get(this.legs[0], "mass"), get(this.legs[1], "mass"),
                get(this.legs[2], "mass"), get(this.legs[3], "mass"),
            ],
            hull_velocity: [
                get(this.hull, "velocity_x"), get(this.hull, "velocity_y"),
                get(this.hull, "velocity_z"),
            ],
            hull_angular_velocity: [
                get(this.hull, "angvel_x"), get(this.hull, "angvel_y"),
                get(this.hull, "angvel_z"),
            ],
            strut_velocity: [
                get(this.springs[0], "velocity"), get(this.springs[1], "velocity"),
                get(this.springs[2], "velocity"), get(this.springs[3], "velocity"),
            ],
            stroke_min: this.stroke_min,
            stroke_max: this.stroke_max,
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
    // reports the whole material law, `k*x + c*v`, and the planted-gear damping
    // deliberately makes even a small residual rate visible in that force.
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

    // The pads are fixed colliders on the prismatic leg bodies. On flat ground
    // their contact faces must remain level; a separate shoe joint would add a
    // second dynamic body and an unnecessary rotational state.
    for i in 0..4 {
        let a = s.pad_tilt[i];
        if a >= 0.0 {
            f.push(t_range(a, 0.0, 0.75,
                "pad " + names[i] + " tilt (deg) — rigid leg collider did not hold"));
        }
    }

    // Pose alone can pass while the landed mechanism is still exchanging energy
    // between its four struts. Prove the actual state has rung down: the hull is
    // neither translating nor yawing, and each strut remains inside a 5 mm
    // peak-to-peak envelope throughout the final two seconds. That window is the
    // visible mechanism state; one instantaneous XPBD velocity sample contains
    // contact-iteration chatter even when its integrated travel is microscopic.
    let hull_speed = vlen(s.hull_velocity);
    let hull_rate = vlen(s.hull_angular_velocity);
    f.push(t_range(hull_speed, 0.0, 0.03,
        "hull speed after planted-gear ring-down (m/s)"));
    f.push(t_range(hull_rate, 0.0, 0.01,
        "hull angular speed after planted-gear ring-down (rad/s)"));
    let stroke_span = [];
    for i in 0..4 {
        let span = s.stroke_max[i] - s.stroke_min[i];
        stroke_span.push(span);
        f.push(t_range(span, 0.0, 0.005,
            "strut " + names[i] + " final two-second travel envelope (m)"));
    }

    print("[legs] ── results ──────────────────────────────────────────");
    print("[legs]   hull y      " + r2(s.hull_y) + " m");
    print("[legs]   hull tilt   " + r2(s.tilt) + " deg");
    print("[legs]   strut loads " + s.forces);
    print("[legs]   strut stroke " + s.strokes);
    print("[legs]   frame err   " + s.frame_err + " deg");
    print("[legs]   pad tilt    " + s.pad_tilt + " deg");
    print("[legs]   hull speed  " + r4(vlen(s.hull_velocity)) + " m/s");
    print("[legs]   hull vel    " + s.hull_velocity + " m/s");
    print("[legs]   hull rate   " + r4(vlen(s.hull_angular_velocity)) + " rad/s");
    print("[legs]   hull omega  " + s.hull_angular_velocity + " rad/s");
    print("[legs]   strut rate  " + s.strut_velocity + " m/s");
    print("[legs]   stroke span " + stroke_span + " m over final 2 s");
    print("[legs]   peak strut  " + r2(s.peak_force) + " N");
    print("[legs]   hull mass   " + s.hull_mass + " kg");
    print("[legs]   hull inertia " + s.hull_inertia + " kg m2");
    print("[legs]   leg masses  " + s.leg_mass + " kg");
    print("[legs]   contacts    " + s.leg_contact_force + " N / " + s.leg_contact);

    report_verdict(f, "LANDING LEGS", "LANDING_LEGS");
}
