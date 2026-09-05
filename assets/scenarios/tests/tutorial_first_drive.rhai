//! Regression harness for the built-in First Drive lesson.
//!
//! The lesson's own `task(me)` is the subject under test. This scenario only
//! observes its public command/event surface and reports a verdict when the
//! lesson's real mission completes. It intentionally does not drive the rover
//! itself: duplicating the autopilot here would allow the test to pass while
//! the built-in lesson was broken.

fn on_start(me) {
    this.possess_count = 0;
    this.port_writes = 0;
    this.verdict_done = false;
    this.elapsed = 0.0;
    this.sun = find("/FirstDrive/Sun");
}

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_FIRST_DRIVE", "PASS");
    } else {
        print("TUTORIAL_FIRST_DRIVE FAIL: " + reason);
        emit("TUTORIAL_FIRST_DRIVE", "FAIL");
    }
}

fn on_event(me, evt) {
    if evt.name == "cmd:PossessVessel" {
        this.possess_count += 1;
    } else if evt.name == "cmd:SetPorts" && this.possess_count > 0 {
        this.port_writes += 1;
    } else if evt.name == "MISSION_FAILED" {
        if !this.verdict_done {
            this.verdict_done = true;
            verdict(false, "the built-in First Drive mission failed");
        }
    } else if evt.name == "MISSION_COMPLETE" {
        let rover = find("/FirstDrive/Rover");
        let flag = find("/FirstDrive/Flag");
        let at_flag = false;
        if rover >= 0 && flag >= 0 {
            at_flag = arrived(rover, world_pos(flag), 6.0);
        }
        let controls_used = this.possess_count > 0 && this.port_writes > 0;
        let sun_forward = world_forward(this.sun);
        let light_is_downward = this.sun >= 0 && sun_forward != () && sun_forward[1] < -0.25;
        if !light_is_downward {
            this.verdict_done = true;
            verdict(false, "the authored scene sun is not emitting down onto the surface");
        } else if !controls_used {
            this.verdict_done = true;
            verdict(false, "autopilot did not use PossessVessel followed by SetPorts");
        } else if !at_flag {
            this.verdict_done = true;
            verdict(false, "mission completed without the rover reaching the flag");
        } else {
            this.verdict_done = true;
            verdict(true, "");
        }
    }
}

// A real verdict is required even when the lesson stalls. This is test-only
// liveness handling; the lesson itself remains event/state driven.
fn on_tick(me) {
    if this.verdict_done { return; }
    this.elapsed += dt();
    if this.elapsed > 60.0 {
        print("TUTORIAL_FIRST_DRIVE timeout: possess=" + this.possess_count +
              " set_ports=" + this.port_writes);
        this.verdict_done = true;
        verdict(false, "lesson did not complete within 60 simulated seconds");
    }
}
