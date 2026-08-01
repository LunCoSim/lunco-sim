//! Regression harness for LunCoSim's Authority Handoff lesson.
//! The lesson's task issues the same possess/release commands exposed by the
//! Command Deck; this observer verifies both commands and the final authority
//! state, without supplying a second control path.

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_AUTHORITY_HANDOFF", "PASS");
    } else {
        print("TUTORIAL_AUTHORITY_HANDOFF FAIL: " + reason);
        emit("TUTORIAL_AUTHORITY_HANDOFF", "FAIL");
    }
}

fn on_start(me) {
    expect_fault("earth_mount_x");
    expect_fault("earth_mount_y");
    expect_fault("earth_mount_z");
    this.possess_count = 0;
    this.release_count = 0;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_event(me, evt) {
    if evt.name == "cmd:PossessVessel" {
        this.possess_count += 1;
    } else if evt.name == "cmd:ReleaseVessel" {
        this.release_count += 1;
    } else if evt.name == "MISSION_FAILED" {
        if !this.done { this.done = true; verdict(false, "the built-in authority lesson failed"); }
    } else if evt.name == "MISSION_COMPLETE" && !this.done {
        let rover = find("/FirstDrive/Rover");
        let returned = false;
        if rover >= 0 { returned = !is_controlled(rover); }
        this.done = true;
        if this.possess_count == 0 { verdict(false, "no PossessVessel command was issued"); return; }
        if this.release_count == 0 { verdict(false, "no ReleaseVessel command was issued"); return; }
        if !returned { verdict(false, "rover remained under human authority"); return; }
        verdict(true, "");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 15.0 {
        this.done = true;
        verdict(false, "lesson did not complete within 15 simulated seconds");
    }
}
