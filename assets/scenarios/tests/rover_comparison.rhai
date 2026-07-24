// Simultaneous three-tier rover comparison over the primitive field in
// `scenes/tests/rover_comparison.usda`.  This is intentionally one shared
// command timeline: the scene compares vehicle properties, never driver timing.

fn on_start(me) {
    this.t = 0.0;
    this.settle_s = 4.0;
    this.run_s = 28.0;
    this.next_sample = 0.0;
    this.names = ["Easy", "Medium", "Awful"];
    this.rovers = [
        find("/RoverComparison/Easy"),
        find("/RoverComparison/Medium"),
        find("/RoverComparison/Awful"),
    ];
    this.prev = [(), (), ()];
    this.distance = [0.0, 0.0, 0.0];
    this.max_tilt = [0.0, 0.0, 0.0];
    this.phase = "settle";
    print("[rover-comparison] identical primitive terrain: cylinders, blocks, ramp");
    print("[rover-comparison] all tiers launch simultaneously after settling");
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    for i in 0..3 {
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
            for i in 0..3 { this.prev[i] = world_pos(this.rovers[i]); }
            this.phase = "drive";
            this.next_sample = this.t;
            print("[rover-comparison] DRIVE: throttle=1.0, steer=0.0, all tiers");
        }
        return;
    }

    for i in 0..3 {
        let rover = this.rovers[i];
        drive(rover, 1.0, 0.0);
        let pos = world_pos(rover);
        this.distance[i] += seg(this.prev[i], pos);
        this.prev[i] = pos;
        let tilt = tilt_deg(rover).abs();
        if tilt > this.max_tilt[i] { this.max_tilt[i] = tilt; }
    }

    if this.t >= this.next_sample {
        let line = "[rover-comparison] t=" + r2(this.t);
        for i in 0..3 {
            line += "  " + this.names[i] + " d=" + r2(this.distance[i]);
            line += " tilt=" + r2(this.max_tilt[i]);
        }
        print(line);
        this.next_sample = this.t + 1.0;
    }

    if this.t >= this.settle_s + this.run_s {
        for rover in this.rovers { brake(rover); }
        let checks = [];
        for i in 0..3 {
            let rover = this.rovers[i];
            checks.push(t_bounded(world_pos(rover), 200.0, this.names[i] + " rover"));
            checks.push(t_true(this.distance[i] > 8.0,
                this.names[i] + " travelled only " + r2(this.distance[i]) + " m"));
            checks.push(t_true(this.max_tilt[i] < 80.0,
                this.names[i] + " overturned (peak tilt " + r2(this.max_tilt[i]) + " deg)"));
        }
        print("[rover-comparison] ── final comparison ─────────────────────");
        for i in 0..3 {
            print("[rover-comparison] " + this.names[i] +
                " distance=" + r2(this.distance[i]) + " m" +
                " peak_tilt=" + r2(this.max_tilt[i]) + " deg");
        }
        report_verdict(checks, "ROVER COMPARISON", "ROVER_COMPARISON");
        this.phase = "done";
    }
}
