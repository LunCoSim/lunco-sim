// Simultaneous rocker-bogie tier and conventional-control comparison over the
// `scenes/tests/rover_comparison.usda`.  This is intentionally one shared
// command timeline: the scene compares vehicle properties, never driver timing.

fn on_start(me) {
    // Keep the comparison readable by default.  Attachment diagnostics remain
    // available through Settings > Debug Visualization when investigating a
    // particular rover.
    cmd("ToggleJointViz", #{ show_joints: false, show_wheel_forces: false });
    this.t = 0.0;
    this.settle_s = 4.0;
    // This is deliberately long enough to exercise the sequence rather than
    // merely reaching its first obstacle.  Headless scene tests run uncapped.
    this.run_s = 64.0;
    this.next_sample = 0.0;
    // ── Subjects are DISCOVERED, not hardcoded ───────────────────────────────
    // The SCENE decides which rovers exist; a scenario that names a fixed seven
    // is asserting against a scene it cannot see. This one did, and four of the
    // seven had been removed from the course — so the run aborted on tick 7 with
    // "missing Conventional rover" and never measured anything. A comparison
    // test that cannot survive the course being re-cut is not measuring the
    // rovers, it is measuring the file.
    //
    // Candidates are the roster this course has ever carried; whichever resolve
    // are the subjects for this run.
    let candidates = [
        ["Easy",              "/RoverComparison/Easy"],
        ["Medium",            "/RoverComparison/Medium"],
        ["Awful",             "/RoverComparison/Awful"],
        ["Conventional",      "/RoverComparison/Conventional"],
        ["Skid4",             "/RoverComparison/SkidFourWheel"],
        ["AckermannRaycast4", "/RoverComparison/AckermannRaycast4"],
        ["AckermannJoint4",   "/RoverComparison/AckermannJoint4"],
    ];
    this.names = [];
    this.rovers = [];
    this.prev = [];
    this.distance = [];
    this.max_tilt = [];
    this.max_grade = [];
    for c in candidates {
        let gid = find(c[1]);
        if gid != () && gid >= 0 {
            this.names.push(c[0]);
            this.rovers.push(gid);
            this.prev.push(());
            this.distance.push(0.0);
            this.max_tilt.push(0.0);
            this.max_grade.push(0);
        }
    }
    this.n = this.rovers.len();

    // ANTI-TRIVIAL GUARD. Discovery must not turn "the scene is empty" into a
    // pass: with nothing to compare, every per-rover check below vacuously
    // succeeds. A comparison needs at least two subjects to be a comparison.
    if this.n < 2 {
        print("ROVER COMPARISON: FAIL — only " + this.n + " of " +
              candidates.len() + " candidate rovers exist in the scene; " +
              "a comparison needs at least two");
        emit("ROVER_COMPARISON", "FAIL");
        this.phase = "done";
        return;
    }
    print("[rover-comparison] subjects (" + this.n + "): " + this.names);
    this.phase = "settle";
    print("[rover-comparison] identical terrain: repeated rounded/square suspension bands, then one continuous 5/10/15/20/25/30/35/40 deg grade");
    print("[rover-comparison] all tiers launch simultaneously after settling");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    // No missing-rover check here: discovery in `on_start` only admits GIDs that
    // resolved, so every subject in `this.rovers` is valid by construction.

    if this.phase == "settle" {
        for rover in this.rovers { brake(rover); }
        if this.t >= this.settle_s {
            for i in 0..this.n { this.prev[i] = world_pos(this.rovers[i]); }
            // Driven DIRECTLY, one shared command for every rover.
            //
            // This used to `EngageAutopilot` and let an authored BT mission
            // steer to waypoint prims under `/RoverComparison/Routes`. Those
            // prims do not exist in this scene and never have, so every rover
            // sat at its spawn and the comparison measured 0.01 m of travel.
            // The course is a straight ladder of suspension bands and grades
            // along -Z, so full throttle and no steer IS the manoeuvre — and it
            // keeps the test self-contained instead of resting on authored
            // content it does not own.
            this.phase = "drive";
            this.next_sample = this.t;
            print("[rover-comparison] DRIVE: full throttle, no steer, all tiers together");
        }
        return;
    }

    for i in 0..this.n {
        let rover = this.rovers[i];
        let at = world_pos(rover);
        // One shared command timeline: the scene compares vehicle properties,
        // never driver timing, so every rover gets the identical input — and
        // every rover STOPS at the same place, the far end of the grade ladder
        // (the 40 deg station sits at z = -37). Driving on past it would only
        // measure how far a vehicle coasts across empty ground, and would trip
        // the scene-bounds check for doing so.
        if at[2] > COURSE_END_Z { drive(rover, 1.0, 0.0); } else { brake(rover); }
        let pos = world_pos(rover);
        this.distance[i] += seg(this.prev[i], pos);
        this.prev[i] = pos;
        let tilt = tilt_deg(rover).abs();
        if tilt > this.max_tilt[i] { this.max_tilt[i] = tilt; }
        let grade = grade_reached(pos[2]);
        if grade > this.max_grade[i] { this.max_grade[i] = grade; }
    }

    if this.t >= this.next_sample {
        let line = "[rover-comparison] t=" + r2(this.t);
        for i in 0..this.n {
            line += "  " + this.names[i] + " d=" + r2(this.distance[i]);
            line += " tilt=" + r2(this.max_tilt[i]) + " grade=" + this.max_grade[i];
        }
        print(line);
        this.next_sample = this.t + 1.0;
    }

    if this.t >= this.settle_s + this.run_s {
        for rover in this.rovers { brake(rover); }
        let checks = [];
        for i in 0..this.n {
            let rover = this.rovers[i];
            checks.push(t_bounded(world_pos(rover), 200.0, this.names[i] + " rover"));
            checks.push(t_true(this.distance[i] > 8.0,
                this.names[i] + " travelled only " + r2(this.distance[i]) + " m"));
        }
        // Per-subject floors, addressed by NAME rather than by position. With a
        // discovered roster, index 6 is not reliably "AckermannJoint4" — and on
        // a three-rover course it is past the end of the array entirely. A named
        // lookup binds each floor to the rover it was written for, and skips
        // quietly when the course does not carry that rover.
        //
        // The tiers deliberately have different stability margins: the easy and
        // medium vehicles must stay upright, while the top-heavy "Awful" vehicle
        // is REPORTED rather than prescribed — its outcome legitimately depends
        // on the selected terrain-grip variant.
        let tilt_caps = [["Easy", 30.0], ["Medium", 80.0]];
        for cap in tilt_caps {
            let i = idx_of(this.names, cap[0]);
            if i >= 0 {
                checks.push(t_true(this.max_tilt[i] < cap[1],
                    cap[0] + " lost its stability margin (peak tilt " +
                    r2(this.max_tilt[i]) + " deg)"));
            }
        }
        // A distance-only gate can turn green before a rover ever enters the
        // grade ladder. These capability floors keep a physics or control
        // regression from masquerading as a comparison.
        let grade_floors = [["Easy", 10], ["AckermannJoint4", 5]];
        for f in grade_floors {
            let i = idx_of(this.names, f[0]);
            if i >= 0 {
                checks.push(t_true(this.max_grade[i] >= f[1],
                    f[0] + " did not reach the " + f[1] + " deg grade (max " +
                    this.max_grade[i] + " deg)"));
            }
        }
        print("[rover-comparison] ── final comparison ─────────────────────");
        for i in 0..this.n {
            print("[rover-comparison] " + this.names[i] +
                " distance=" + r2(this.distance[i]) + " m" +
                " peak_tilt=" + r2(this.max_tilt[i]) + " deg" +
                " max_grade=" + this.max_grade[i] + " deg");
        }
        report_verdict(checks, "ROVER COMPARISON", "ROVER_COMPARISON");
        // Only the comparison vehicles stop.  Do not pause the global world:
        // avatar/UI input must remain live after a pass or failure so the user
        // can inspect the result and continue navigating the scene.
        this.phase = "done";
    }
}

// Index of `want` in `names`, or -1 when the course does not carry it. Keeps the
// capability floors bound to the rover each was written for: with a discovered
// roster, a positional index silently shifts onto a different vehicle.
fn idx_of(names, want) {
    for i in 0..names.len() {
        if names[i] == want { return i; }
    }
    return -1;
}

// Past the last authored station (40 deg at z = -37), with a short run-out so a
// rover finishes the ramp rather than braking on it.
const COURSE_END_Z = -42.0;

// The grade boundaries are authored course stations, not a control law.  This
// scenario test records the highest actual ramp entered by each rover.
fn grade_reached(z) {
    if z <= -37.0 { return 40; }
    if z <= -34.0 { return 35; }
    if z <= -31.0 { return 30; }
    if z <= -28.0 { return 25; }
    if z <= -25.0 { return 20; }
    if z <= -22.0 { return 15; }
    if z <= -19.0 { return 10; }
    if z <= -16.0 { return 5; }
    return 0;
}

// A reload or manual stop can interrupt the run before its terminal pause.
// Release the command surface in that case too, so no vehicle inherits a
// previous full-throttle command into the next scenario.
fn on_stop(me) {
    for rover in this.rovers {
        if rover != () && rover >= 0 {
            brake(rover);

        }
    }
}
