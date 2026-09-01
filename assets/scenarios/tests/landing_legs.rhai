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
// compression along the hull-to-foot axis. Each round pad is a child collider
// of its leg body, so it cannot rotate independently or turn onto its rim.
//
// So the assertions are about the vehicle STILL BEING A VEHICLE after the drop:
// bodies at finite, bounded positions, the hull level and at rest, all four
// struts sharing the load, and the rigid leg bodies keeping every round pad level.
// A silent frame regression fails every one of them.
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
    // Pin this diagnostic fixture to Avian's documented production contract.
    // This is test policy, so it can be changed and rerun without rebuilding
    // the Rust host.
    if !set_physics_substeps(8) {
        print("[legs] unable to set Avian substeps to 8");
    }
    print("[legs] avian_substeps=" + physics_substeps());
    this.t = 0.0;
    this.sample_s = 0.5;
    this.next_sample = 0.0;
    // Stability is a physical observation, not a qualification deadline. A
    // candidate must continuously satisfy support, load, velocity, and stroke
    // envelope conditions; any violation restarts the observation window.
    this.stable_window_s = 2.0;
    this.stable_s = 0.0;
    this.stable_stroke_min = [1000000.0, 1000000.0, 1000000.0, 1000000.0];
    this.stable_stroke_max = [-1000000.0, -1000000.0, -1000000.0, -1000000.0];
    this.full_stroke_min = [1000000.0, 1000000.0, 1000000.0, 1000000.0];
    this.full_stroke_max = [-1000000.0, -1000000.0, -1000000.0, -1000000.0];
    this.full_stroke_samples = 0;
    this.slider_axes = leg_slider_axes();
    this.start_hull_rotation = ();
    this.leg_start_relative = [];
    this.alignment_samples = 0;
    this.max_lateral_error = 0.0;
    this.max_axial_mismatch = 0.0;

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
    let hull_pos = world_pos(this.hull);
    this.start_hull_rotation = world_rotation(this.hull);
    for leg in this.legs {
        let leg_pos = world_pos(leg);
        if hull_pos == () || leg_pos == () { this.leg_start_relative.push(()); }
        else { this.leg_start_relative.push(vsub(leg_pos, hull_pos)); }
    }
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
    print("[legs] dropping from 3 m; waiting for physical convergence");
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
    let a0 = leg_slider_axes();
    let a1 = [0.0, -1.0, 0.0];
    let out = [];
    for i in 0..legs.len() {
        let d = joint_lock_error_deg(hull, a0[i], legs[i], a1);
        if d < 0.0 { out.push(()); } else { out.push(r4(d)); }
    }
    out
}

/// The four authored slider axes, expressed in the hull frame. This is the
/// same `localRot0 * physics:axis` contract used by `frame_errors` and by the
/// position-residual measurement below.
fn leg_slider_axes() {
    let s = 0.34202014;
    let c = 0.93969262;
    [[s, -c, 0.0], [-s, -c, 0.0],
     [0.0, -c, s], [0.0, -c, -s]]
}

/// Measure one leg against the joint's authored geometric frame. The initial
/// relative pose is rotated with the hull so hull motion is not mistaken for
/// slider error. The result is `[lateral_residual_m, axial_stroke_error_m]`.
fn leg_alignment(hull, leg, index, start_relative, start_hull_rotation, axes, stroke) {
    let hq = world_rotation(hull);
    let hp = world_pos(hull);
    let lp = world_pos(leg);
    if hq == () || hp == () || lp == () || start_hull_rotation == () ||
       start_relative[index] == () { return (); }

    let start_local = qrot([-start_hull_rotation[0], -start_hull_rotation[1],
                            -start_hull_rotation[2], start_hull_rotation[3]],
                           start_relative[index]);
    let expected_delta = qrot(hq, start_local);
    let actual_delta = vsub(lp, hp);
    let residual = vsub(actual_delta, expected_delta);
    let axis = qrot(hq, axes[index]);
    let axial = vdot(residual, axis);
    let lateral = vsub(residual, vscale(axis, axial));
    let axial_error = if stroke == () { () } else { (axial - stroke).abs() };
    [vlen(lateral), axial_error]
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
    let frame = frame_errors(this.hull, this.legs);

    for i in 0..4 {
        if x[i] != () {
            if x[i] < this.full_stroke_min[i] { this.full_stroke_min[i] = x[i]; }
            if x[i] > this.full_stroke_max[i] { this.full_stroke_max[i] = x[i]; }
            this.full_stroke_samples += 1;
        }
        let alignment = leg_alignment(this.hull, this.legs[i], i,
                                      this.leg_start_relative,
                                      this.start_hull_rotation,
                                      this.slider_axes, x[i]);
        if alignment != () {
            this.alignment_samples += 1;
            if alignment[0] > this.max_lateral_error {
                this.max_lateral_error = alignment[0];
            }
            if alignment[1] != () && alignment[1] > this.max_axial_mismatch {
                this.max_axial_mismatch = alignment[1];
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
        print("[legs]     frame_err_deg=" + frame);
        print("[legs]     pad_tilt_deg=" + pad_tilts(this.pads, this.surface_up));
        print("[legs]     leg_contact_N=" + leg_contact_forces(this.legs)
            + "  contact=" + leg_contact_state(this.legs));
        this.next_sample = this.t + this.sample_s;
    }

    // A landed mechanism is stable only when every physical support and motion
    // observable agrees at the same time. This is a state-based convergence
    // detector: it has no fixed drop/settling schedule and cannot pass a late
    // oscillation by sampling one convenient tick.
    let hull_velocity = [get(this.hull, "velocity_x"), get(this.hull, "velocity_y"),
                         get(this.hull, "velocity_z")];
    let hull_angular_velocity = [get(this.hull, "angvel_x"), get(this.hull, "angvel_y"),
                                 get(this.hull, "angvel_z")];
    let stable_sample = vlen(hull_velocity) <= 0.03 &&
                        vlen(hull_angular_velocity) <= 0.01;
    for i in 0..4 {
        let contact = get(this.legs[i], "contact");
        stable_sample = stable_sample && contact != () && contact > 0.5 &&
                        f[i] != () && f[i] > 100.0 && x[i] != () &&
                        x[i] < -0.005 && x[i] >= -0.802 && x[i] <= 0.002;
    }

    if stable_sample {
        this.stable_s += dt();
        for i in 0..4 {
            if x[i] < this.stable_stroke_min[i] { this.stable_stroke_min[i] = x[i]; }
            if x[i] > this.stable_stroke_max[i] { this.stable_stroke_max[i] = x[i]; }
            if this.stable_stroke_max[i] - this.stable_stroke_min[i] > 0.005 {
                stable_sample = false;
            }
        }
    }

    if !stable_sample {
        this.stable_s = 0.0;
        this.stable_stroke_min = [1000000.0, 1000000.0, 1000000.0, 1000000.0];
        this.stable_stroke_max = [-1000000.0, -1000000.0, -1000000.0, -1000000.0];
    }

    if stable_sample && this.stable_s >= this.stable_window_s {
        this.phase = "done";
        // An explicit map, NOT `this`: helpers never see `this`, and passing a
        // plain map keeps the verdict a pure function of the measurements.
        verdict(#{
            substeps: physics_substeps(),
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
            frame_err: frame,
            alignment_samples: this.alignment_samples,
            max_lateral_error: this.max_lateral_error,
            max_axial_mismatch: this.max_axial_mismatch,
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
            hull_velocity: hull_velocity,
            hull_angular_velocity: hull_angular_velocity,
            strut_velocity: [
                get(this.springs[0], "velocity"), get(this.springs[1], "velocity"),
                get(this.springs[2], "velocity"), get(this.springs[3], "velocity"),
            ],
            stroke_min: this.stable_stroke_min,
            stroke_max: this.stable_stroke_max,
            full_stroke_min: this.full_stroke_min,
            full_stroke_max: this.full_stroke_max,
            full_stroke_samples: this.full_stroke_samples,
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
//                    an asymmetric strut. The final verdict waits for physical
//                    convergence instead of selecting a fixed elapsed time.
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
//   joint geometry     the direct frame residual stays small in the lateral and
//                    axial directions throughout the run. This observes the
//                    prismatic body's actual geometry, rather than inferring
//                    constraint health from hull height or spring force alone.
fn verdict(s) {
    let f = [];

    f.push(t_true(s.substeps == 8,
        "landing-gear fixture must run at Avian's eight-substep default, got " + s.substeps));
    f.push(t_true(s.alignment_samples > 0,
        "no leg alignment samples were captured"));
    f.push(t_true(s.full_stroke_samples > 0,
        "no full-run suspension samples were captured"));
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

    // Position residuals are measured against the two joint frames, not against
    // world-up or a visual mesh. A lateral residual means the leg is escaping
    // the prismatic axis; a stroke mismatch means the port and the body pose no
    // longer describe the same physical DOF.
    f.push(t_range(s.max_lateral_error, 0.0, 0.01,
        "maximum lateral leg residual (m)"));
    f.push(t_range(s.max_axial_mismatch, 0.0, 0.01,
        "maximum joint stroke/pose mismatch (m)"));
    // The authored stops are the physical travel boundary. Check the complete
    // landing, including impact, so a leg cannot pass by ending in range after
    // crossing a stop during the drop.
    if s.full_stroke_samples > 0 {
        for i in 0..4 {
            f.push(t_range(s.full_stroke_min[i], -0.802, 0.002,
                "strut " + names[i] + " full-run lower travel limit (m)"));
            f.push(t_range(s.full_stroke_max[i], -0.002, 0.002,
                "strut " + names[i] + " full-run upper travel limit (m)"));
        }
    }

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
    // between its four struts. The convergence detector above proves the actual
    // state has rung down: the hull is neither translating nor yawing, every foot
    // is carrying load, and each strut remains inside a 5 mm peak-to-peak envelope
    // throughout the observed stable window. One instantaneous XPBD velocity
    // sample contains contact-iteration chatter even when its integrated travel
    // is microscopic.
    let hull_speed = vlen(s.hull_velocity);
    let hull_rate = vlen(s.hull_angular_velocity);
    f.push(t_range(hull_speed, 0.0, 0.03,
        "hull speed at physical convergence (m/s)"));
    f.push(t_range(hull_rate, 0.0, 0.01,
        "hull angular speed at physical convergence (rad/s)"));
    let stroke_span = [];
    for i in 0..4 {
        let span = s.stroke_max[i] - s.stroke_min[i];
        stroke_span.push(span);
        f.push(t_range(span, 0.0, 0.005,
            "strut " + names[i] + " convergence travel envelope (m)"));
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
    print("[legs]   stroke span " + stroke_span + " m over stable convergence window");
    print("[legs]   max lateral " + r4(s.max_lateral_error) + " m");
    print("[legs]   max axial mismatch " + r4(s.max_axial_mismatch) + " m");
    print("[legs]   full stroke " + s.full_stroke_min + " .. " + s.full_stroke_max + " m");
    print("[legs]   peak strut  " + r2(s.peak_force) + " N");
    print("[legs]   hull mass   " + s.hull_mass + " kg");
    print("[legs]   hull inertia " + s.hull_inertia + " kg m2");
    print("[legs]   leg masses  " + s.leg_mass + " kg");
    print("[legs]   contacts    " + s.leg_contact_force + " N / " + s.leg_contact);

    report_verdict(f, "LANDING LEGS", "LANDING_LEGS");
}
