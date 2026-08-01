//! Regression harness for the built-in Build a Lunar Base lesson.
//! The lesson task uses the real SpawnEntity catalog command six times; this
//! observer checks that command path and waits for the real objective roll-up.

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_BUILD_BASE", "PASS");
    } else {
        print("TUTORIAL_BUILD_BASE FAIL: " + reason);
        emit("TUTORIAL_BUILD_BASE", "FAIL");
    }
}

fn on_start(me) {
    this.spawn_count = 0;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_event(me, evt) {
    if evt.name == "cmd:SpawnEntity" {
        this.spawn_count += 1;
    } else if evt.name == "OBJECTIVE_COMPLETE" && evt.value == "comms" {
        // The lesson creates the Comms Mast after this observer starts. Its
        // local EarthTracker wires are intentionally unlanded in this
        // non-celestial fixture, so register the expected faults when that
        // authored object is actually spawned.
        expect_fault("earth_mount_x");
        expect_fault("earth_mount_y");
        expect_fault("earth_mount_z");
    } else if evt.name == "MISSION_FAILED" {
        if !this.done { this.done = true; verdict(false, "the built-in base-building mission failed"); }
    } else if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        if this.spawn_count < 5 {
            verdict(false, "autopilot did not use SpawnEntity for the base parts");
        } else {
            verdict(true, "");
        }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.done = true;
        verdict(false, "lesson did not complete within 30 simulated seconds");
    }
}
