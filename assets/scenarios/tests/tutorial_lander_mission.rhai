//! Regression harness for the built-in Lander & Rover Mission lesson.
//! The lesson observes the scene's descent/deployment events, then drives the
//! rover with the same PossessVessel + SetPorts path as the human controls.

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_LANDER_MISSION", "PASS");
    } else {
        print("TUTORIAL_LANDER_MISSION FAIL: " + reason);
        emit("TUTORIAL_LANDER_MISSION", "FAIL");
    }
}

fn on_start(me) {
    this.possess_count = 0;
    this.port_writes = 0;
    this.deployed = false;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.deployed = true;
    } else if evt.name == "cmd:PossessVessel" && this.deployed {
        this.possess_count += 1;
    } else if evt.name == "cmd:SetPorts" && this.deployed && this.possess_count > 0 {
        this.port_writes += 1;
    } else if evt.name == "MISSION_FAILED" {
        if !this.done { this.done = true; verdict(false, "the built-in lander mission failed"); }
    } else if evt.name == "MISSION_COMPLETE" && !this.done {
        let rover = find("/LanderTest/SkidRover");
        let goal = find("/LanderTest/RoverTarget3");
        let arrived = false;
        if rover >= 0 && goal >= 0 { arrived = distance(rover, goal) < 6.0; }
        let used_controls = false;
        if this.deployed && this.possess_count > 0 { used_controls = this.port_writes > 0; }
        this.done = true;
        if !used_controls { verdict(false, "autopilot did not drive after rover deployment"); return; }
        if !arrived { verdict(false, "mission completed away from the final waypoint"); return; }
        verdict(true, "");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 180.0 {
        this.done = true;
        verdict(false, "lesson did not complete within 180 simulated seconds");
    }
}
