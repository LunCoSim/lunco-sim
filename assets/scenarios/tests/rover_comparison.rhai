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
    this.names = ["Easy", "Medium", "Awful", "Conventional", "Skid4", "AckermannRaycast4", "AckermannJoint4"];
    this.rovers = [
        find("/RoverComparison/Easy"),
        find("/RoverComparison/Medium"),
        find("/RoverComparison/Awful"),
        find("/RoverComparison/Conventional"),
        find("/RoverComparison/SkidFourWheel"),
        find("/RoverComparison/AckermannRaycast4"),
        find("/RoverComparison/AckermannJoint4"),
    ];
    this.prev = [(), (), (), (), (), (), ()];
    this.distance = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    this.max_tilt = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    this.max_grade = [0, 0, 0, 0, 0, 0, 0];
    this.phase = "settle";
    print("[rover-comparison] identical terrain: repeated rounded/square suspension bands, then one continuous 5/10/15/20/25/30/35/40 deg grade");
    print("[rover-comparison] all tiers launch simultaneously after settling");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    for i in 0..7 {
        let rover = this.rovers[i];
        if rover == () || rover < 0 {
            print("ROVER COMPARISON: FAIL — missing " + this.names[i] + " rover");
            emit("ROVER_COMPARISON", "FAIL");
            this.phase = "done";
            return;
        }
    }

    if this.phase == "settle" {
        for rover in this.rovers { brake(rover); }
        if this.t >= this.settle_s {
            for i in 0..7 { this.prev[i] = world_pos(this.rovers[i]); }
            // Every rover carries a BT.CPP mission whose spatial leaves target
            // the USD waypoint prims under /RoverComparison/Routes.  Engaging
            // without a JSON spec lets that authored program be the single
            // control source.
            for i in 0..7 {
                cmd("EngageAutopilot", #{ vessel: this.rovers[i], index: i });
            }
            this.phase = "drive";
            this.next_sample = this.t;
            print("[rover-comparison] DRIVE: terrain-following autopilot routes, all tiers");
        }
        return;
    }

    for i in 0..7 {
        let rover = this.rovers[i];
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
        for i in 0..7 {
            line += "  " + this.names[i] + " d=" + r2(this.distance[i]);
            line += " tilt=" + r2(this.max_tilt[i]) + " grade=" + this.max_grade[i];
        }
        print(line);
        this.next_sample = this.t + 1.0;
    }

    if this.t >= this.settle_s + this.run_s {
        for rover in this.rovers {
            brake(rover);
            cmd("DisengageAutopilot", #{ vessel: rover });
        }
        let checks = [];
        for i in 0..7 {
            let rover = this.rovers[i];
            checks.push(t_bounded(world_pos(rover), 200.0, this.names[i] + " rover"));
            checks.push(t_true(this.distance[i] > 8.0,
                this.names[i] + " travelled only " + r2(this.distance[i]) + " m"));
        }
        // The three variants deliberately have different stability margins.  The
        // easy and medium vehicles must stay upright. The top-heavy "Awful"
        // vehicle is reported as a comparison result, not prescribed to roll:
        // its outcome legitimately depends on the selected terrain-grip variant.
        checks.push(t_true(this.max_tilt[0] < 30.0,
            "Easy lost its stability margin (peak tilt " + r2(this.max_tilt[0]) + " deg)"));
        checks.push(t_true(this.max_tilt[1] < 80.0,
            "Medium overturned (peak tilt " + r2(this.max_tilt[1]) + " deg)"));
        // A distance-only gate can turn green before a rover ever enters the
        // grade ladder.  Keep two representative capability floors so a
        // physics or control regression cannot masquerade as a comparison.
        checks.push(t_true(this.max_grade[0] >= 10,
            "Easy did not reach the 10 deg grade (max " + this.max_grade[0] + " deg)"));
        checks.push(t_true(this.max_grade[6] >= 5,
            "AckermannJoint4 did not reach the 5 deg grade (max " + this.max_grade[6] + " deg)"));
        print("[rover-comparison] ── final comparison ─────────────────────");
        for i in 0..7 {
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

// The grade boundaries are authored course stations, not a control law.  This
// scenario test records the highest actual ramp entered by each rover.
fn grade_reached(z) {
    if z <= -19.0 { return 40; }
    if z <= -18.0 { return 35; }
    if z <= -17.0 { return 30; }
    if z <= -16.0 { return 25; }
    if z <= -15.0 { return 20; }
    if z <= -14.0 { return 15; }
    if z <= -13.0 { return 10; }
    if z <= -12.0 { return 5; }
    return 0;
}

// A reload or manual stop can interrupt the run before its terminal pause.
// Release the command surface in that case too, so no vehicle inherits a
// previous full-throttle command into the next scenario.
fn on_stop(me) {
    for rover in this.rovers {
        if rover != () && rover >= 0 {
            brake(rover);
            cmd("DisengageAutopilot", #{ vessel: rover });
        }
    }
}
