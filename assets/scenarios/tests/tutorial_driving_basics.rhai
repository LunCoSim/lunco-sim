//! Regression harness for the built-in Moon Driving Basics lesson.
//! The lesson script supplies the unattended drive; this observer only checks
//! that it used the same possession and SetPorts surface as a human driver.

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_DRIVING_BASICS", "PASS");
    } else {
        print("TUTORIAL_DRIVING_BASICS FAIL: " + reason);
        emit("TUTORIAL_DRIVING_BASICS", "FAIL");
    }
}

fn on_start(me) {
    expect_fault("earth_mount_x");
    expect_fault("earth_mount_y");
    expect_fault("earth_mount_z");
    this.possess_count = 0;
    this.port_writes = 0;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_event(me, evt) {
    if evt.name == "cmd:PossessVessel" {
        this.possess_count += 1;
    } else if evt.name == "cmd:SetPorts" && this.possess_count > 0 {
        this.port_writes += 1;
    } else if evt.name == "MISSION_FAILED" {
        if !this.done { this.done = true; verdict(false, "the built-in driving mission failed"); }
    } else if evt.name == "MISSION_COMPLETE" && !this.done {
        let rover = find("/DrivingBasics/Rover");
        let flag = find("/DrivingBasics/Flag");
        let arrived = rover >= 0 && flag >= 0 && distance(rover, flag) < 6.0;
        let used_controls = this.possess_count > 0 && this.port_writes > 0;
        this.done = true;
        if !used_controls { verdict(false, "autopilot did not use PossessVessel and SetPorts"); }
        else if !arrived { verdict(false, "mission completed away from the finish flag"); }
        else { verdict(true, ""); }
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 90.0 {
        this.done = true;
        verdict(false, "lesson did not complete within 90 simulated seconds");
    }
}
