// ── Rover tiers over one identical obstacle course ──────────────────────────
//
// TEST-ONLY. Runs against `scenes/tests/rover_comparison.usda`.
//
// WHAT IT GUARDS. The scene lays three NUMERICALLY IDENTICAL lanes at x = -10,
// 0 and +10 — same ridges, same blocks, same ramps, differing only in X — and
// puts one rover tier at the head of each. Every difference in the result is
// therefore a difference between the VEHICLES. That invariant is the test.
//
// ── The scenario owns the manoeuvre; the scene owns the course ──────────────
//
// Two things this file must not restate, because the scene already says them:
//
//   THE ROUTE      is not scene content. It is a property of the manoeuvre under
//                  test, so it is built here from where each rover ACTUALLY
//                  settled — one waypoint at the far end of that rover's own
//                  lane — and handed to the autopilot. Same pattern as
//                  `autopilot_hold.rhai`. This file previously engaged an
//                  autopilot against waypoint prims under
//                  `/RoverComparison/Routes` that do not exist and never have,
//                  so every rover sat at its spawn and the comparison measured
//                  1 cm of travel.
//
//   THE STATIONS   are the obstacle prims themselves, DISCOVERED by walking
//                  `Lanes` and reading each one's position. A hardcoded z-to-
//                  grade table is the scene's geometry copied into a script that
//                  cannot see it — and this file's table described an eight-step
//                  grade ladder the scene does not author. Progress is now
//                  counted in stations actually passed, which is what "identical
//                  obstacle sequence, three tiers" is asking about anyway.
//
// ── Why the autopilot and not raw throttle ──────────────────────────────────
//
// Because the manoeuvre is "traverse the lane", and only a controller does that.
// Full throttle with no steering is a different experiment: it leaves the lane
// (nothing holds x), it does not stop at the end (nothing knows where the end
// is), and it hits the obstacles far harder than a traversing rover would, which
// turns a suspension comparison into a rollover comparison. Measured that way the
// easy tier ran 1.8 km off the course and the medium tier flipped — neither
// number is about the vehicles.
//
// ── HOW FAILURE IS SIGNALLED ────────────────────────────────────────────────
//
// `emit("ROVER_COMPARISON", "PASS"|"FAIL")`.

// Run-out past the last discovered station, so arrival means the whole authored
// sequence is behind the rover. The END of the course is not a constant here: it
// is derived from the stations the scene actually carries, so re-cutting the
// course moves the finish line with it.
const COURSE_RUNOUT_M = 6.0;

// `engage_patrol`'s `speed` is a THROTTLE FRACTION, not a velocity — the
// autopilot scales it by its own approach law (`throttle = speed * f(heading)`).
// 0.6 is the prelude default and a controlled traverse pace: enough to climb the
// authored grades, far from the full-throttle charge that turns a suspension
// comparison into a rollover comparison.
const TRAVERSE_THROTTLE = 0.6;

// A lane is 20 m from its neighbours; this claims obstacles for the lane whose
// rover is nearest without ever reaching the next one.
const LANE_HALF_WIDTH = 5.0;

fn on_start(me) {
    cmd("ToggleJointViz", #{ show_joints: false, show_wheel_forces: false });
    this.t = 0.0;
    this.settle_s = 4.0;
    // Long enough for the slowest tier to traverse the ~90 m course at
    // TRAVERSE_THROTTLE, with margin for the obstacles. Headless tests run uncapped.
    this.run_s = 96.0;
    this.next_sample = 0.0;

    // ── Subjects are DISCOVERED, not hardcoded ───────────────────────────────
    // The SCENE decides which rovers exist; a scenario that names a fixed seven
    // is asserting against a scene it cannot see. This one did, and four of the
    // seven had been removed from the course — so the run aborted on tick 7 with
    // "missing Conventional rover" and never measured anything.
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
    this.passed = [];
    this.lane_stations = [];
    for c in candidates {
        let gid = find(c[1]);
        if gid != () && gid >= 0 {
            this.names.push(c[0]);
            this.rovers.push(gid);
            this.prev.push(());
            this.distance.push(0.0);
            this.max_tilt.push(0.0);
            this.passed.push(0);
            this.lane_stations.push([]);
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

    // The course, read off the scene. Each entry is [gid, x, z] so the lane split
    // and the progress count below are both plain arithmetic on authored poses.
    this.stations = [];
    let lanes = find("/RoverComparison/Lanes");
    if lanes != () && lanes >= 0 {
        for gid in children(lanes) {
            let p = world_pos(gid);
            if p != () { this.stations.push([gid, p[0], p[2]]); }
        }
    }
    // ANTI-TRIVIAL GUARD, the course half. Zero stations would make "passed all
    // of them" true for every rover, including one that never moved.
    if this.stations.len() < 3 {
        print("ROVER COMPARISON: FAIL — found " + this.stations.len() +
              " obstacle(s) under /RoverComparison/Lanes; this course authors " +
              "an obstacle sequence and a comparison over an empty one is vacuous");
        emit("ROVER_COMPARISON", "FAIL");
        this.phase = "done";
        return;
    }

    print("[rover-comparison] subjects (" + this.n + "): " + this.names);
    print("[rover-comparison] course: " + this.stations.len() +
          " obstacle prim(s) across the lanes, discovered from the scene");
    print("[rover-comparison] identical lanes at x = -10 / 0 / +10; all tiers launch together");
    this.phase = "settle";
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();

    if this.phase == "settle" {
        for rover in this.rovers { brake(rover); }
        if this.t >= this.settle_s {
            for i in 0..this.n {
                let rover = this.rovers[i];
                let at = world_pos(rover);
                this.prev[i] = at;
                // This rover's lane, and the stations standing in it. Claimed by
                // proximity to where the rover SETTLED, so a lane that moves in
                // the scene takes its obstacles and its rover with it.
                let mine = [];
                for s in this.stations {
                    if (s[1] - at[0]).abs() <= LANE_HALF_WIDTH { mine.push(s[2]); }
                }
                this.lane_stations[i] = mine;
                // The route: one waypoint at the end of this rover's own lane,
                // on its own settled x, so every tier drives the same leg.
                // The finish line: past this lane's LAST station, so the leg
                // covers the authored sequence and nothing beyond it.
                let last_z = 0.0;
                for sz in mine {
                    if sz < last_z { last_z = sz; }
                }
                let end_z = last_z - COURSE_RUNOUT_M;
                let goal = [at[0], at[1], end_z];
                engage_patrol(rover, [goal], TRAVERSE_THROTTLE, (), ());
                print("[rover-comparison] " + this.names[i] + " lane x=" + r2(at[0]) +
                      "  " + mine.len() + " station(s)  -> z=" + r2(end_z));
            }
            this.phase = "drive";
            this.next_sample = this.t;
            print("[rover-comparison] DRIVE: autopilot traverse, one leg, all tiers together");
        }
        return;
    }

    // No drive()/brake() from here on: whatever moves these rovers is the
    // autopilot, and the tiers must differ only in how they cope with the course.
    for i in 0..this.n {
        let pos = world_pos(this.rovers[i]);
        // Once the escape guard has removed a rover there is nothing left to
        // sample; its last good figures stand and the verdict names it below.
        if pos == () { continue; }
        this.distance[i] += seg(this.prev[i], pos);
        this.prev[i] = pos;
        let tilt = tilt_deg(this.rovers[i]).abs();
        if tilt > this.max_tilt[i] { this.max_tilt[i] = tilt; }
        // Travel is toward -Z, so a station is behind the rover once the rover's
        // z has gone below the station's.
        let done = 0;
        for sz in this.lane_stations[i] {
            if pos[2] < sz { done += 1; }
        }
        if done > this.passed[i] { this.passed[i] = done; }
    }

    if this.t >= this.next_sample {
        let line = "[rover-comparison] t=" + r2(this.t);
        for i in 0..this.n {
            line += "  " + this.names[i] + " d=" + r2(this.distance[i]);
            line += " tilt=" + r2(this.max_tilt[i]);
            line += " st=" + this.passed[i] + "/" + this.lane_stations[i].len();
        }
        print(line);
        this.next_sample = this.t + 2.0;
    }

    if this.t >= this.settle_s + this.run_s {
        for rover in this.rovers { this.clear_patrol(rover); }
        let checks = [];
        for i in 0..this.n {
            let rover = this.rovers[i];
            // A rover the physics escape guard removed has no position at all.
            // That is a RESULT — the vehicle left the world — and it has to be
            // reported as one: passing `()` into the bounds check throws, which
            // kills the scenario between here and `report_verdict` and makes
            // `scene_test` say "no verdict" instead of naming the failure. This
            // file did exactly that, looping past its own end time for 300 s.
            let at = world_pos(rover);
            if at == () {
                checks.push(this.names[i] + " rover left the world entirely — no " +
                    "pose remains, so the physics escape guard removed it after " +
                    r2(this.distance[i]) + " m of travel");
                continue;
            }
            checks.push(t_bounded(at, 200.0, this.names[i] + " rover"));
            // LIVENESS, per subject: the autopilot claimed it and it traversed.
            // Stated against the course rather than as a metre count, so it
            // follows the scene if the lanes are re-cut.
            checks.push(t_true(this.passed[i] > 0,
                this.names[i] + " passed none of its " + this.lane_stations[i].len() +
                " lane obstacles (travelled " + r2(this.distance[i]) + " m) — it " +
                "never entered the course"));
        }

        // Per-subject expectations, addressed by NAME rather than by position.
        // With a discovered roster, index 6 is not reliably "AckermannJoint4" —
        // and on a three-rover course it is past the end of the array entirely.
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
        // The capability floor: the easy tier is the one that must get through
        // the whole authored sequence. A liveness gate alone can go green on a
        // rover that cleared one ridge and stalled.
        let i_easy = idx_of(this.names, "Easy");
        if i_easy >= 0 {
            let want = this.lane_stations[i_easy].len();
            checks.push(t_true(this.passed[i_easy] >= want,
                "Easy passed " + this.passed[i_easy] + " of " + want +
                " lane obstacles — the easy tier is the one expected to clear the " +
                "whole authored sequence"));
        }

        print("[rover-comparison] ── final comparison ─────────────────────");
        for i in 0..this.n {
            print("[rover-comparison] " + this.names[i] +
                " distance=" + r2(this.distance[i]) + " m" +
                " peak_tilt=" + r2(this.max_tilt[i]) + " deg" +
                " stations=" + this.passed[i] + "/" + this.lane_stations[i].len());
        }
        report_verdict(checks, "ROVER COMPARISON", "ROVER_COMPARISON");
        // Only the comparison vehicles stop.  Do not pause the global world:
        // avatar/UI input must remain live after a pass or failure so the user
        // can inspect the result and continue navigating the scene.
        this.phase = "done";
    }
}

// Index of `want` in `names`, or -1 when the course does not carry it. Keeps the
// per-subject expectations bound to the rover each was written for: with a
// discovered roster, a positional index silently shifts onto a different vehicle.
fn idx_of(names, want) {
    for i in 0..names.len() {
        if names[i] == want { return i; }
    }
    return -1;
}

// A reload or manual stop can interrupt the run before its terminal release.
// Drop the autopilot in that case too, so no vehicle inherits a patrol into the
// next scenario.
fn on_stop(me) {
    for rover in this.rovers {
        if rover != () && rover >= 0 {
            this.clear_patrol(rover);
        }
    }
}
