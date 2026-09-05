//! Regression harness for the built-in Slopes & Tip-Over lesson.
//! It requires the lesson's real tilt objective as well as its final arrival.

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_SLOPES_TIPOVER", "PASS");
    } else {
        print("TUTORIAL_SLOPES_TIPOVER FAIL: " + reason);
        emit("TUTORIAL_SLOPES_TIPOVER", "FAIL");
    }
}

fn on_start(me) {
    this.possess_count = 0;
    this.port_writes = 0;
    this.edge_seen = false;
    this.done = false;
    this.elapsed = 0.0;
}

fn on_event(me, evt) {
    if evt.name == "cmd:PossessVessel" {
        this.possess_count += 1;
    } else if evt.name == "cmd:SetPorts" && this.possess_count > 0 {
        this.port_writes += 1;
    } else if evt.name == "OBJECTIVE_COMPLETE" && evt.value == "feel_the_edge" {
        this.edge_seen = true;
    } else if evt.name == "MISSION_FAILED" {
        if !this.done { this.done = true; verdict(false, "the built-in slope mission failed"); }
    } else if evt.name == "MISSION_COMPLETE" && !this.done {
        let rover = find("/SlopeTest/Rover");
        let marker = find("/SlopeTest/Marker");
        let reached_marker = false;
        if rover >= 0 && marker >= 0 {
            reached_marker = arrived(rover, world_pos(marker), 6.0);
        }
        let used_controls = this.possess_count > 0 && this.port_writes > 0;
        this.done = true;
        if !this.edge_seen { verdict(false, "the autopilot skipped the tilt objective"); return; }
        if !used_controls { verdict(false, "autopilot did not use PossessVessel and SetPorts"); return; }
        if !reached_marker { verdict(false, "mission completed away from the top marker"); return; }
        verdict(true, "");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 120.0 {
        print("TUTORIAL_SLOPES_TIPOVER timeout rover=" + world_pos(find("/SlopeTest/Rover")) +
              " tilt=" + tilt_deg(find("/SlopeTest/Rover")) +
              " possess=" + this.possess_count + " set_ports=" + this.port_writes);
        this.done = true;
        verdict(false, "lesson did not complete within 120 simulated seconds");
    }
}
