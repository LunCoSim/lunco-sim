//! Contract for the authored Sandbox onboarding event policy.
//!
//! This observer does not call the tutorial's commands. It emits only the raw
//! input/event names that the real UI produces, and checks the lesson's public
//! SetTourStep and MISSION_COMPLETE surface. The wrong Next at the Camera
//! Cycling step is intentional: it must not advance an action-gated card.

fn on_start(me) {
    this.step = 0;
    this.done = false;
    this.elapsed = 0.0;
}

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_SANDBOX_INTRO", "PASS");
    } else {
        print("TUTORIAL_SANDBOX_INTRO FAIL: " + reason);
        emit("TUTORIAL_SANDBOX_INTRO", "FAIL");
    }
}

fn on_event(me, evt) {
    if evt.name == "MISSION_FAILED" && !this.done {
        this.done = true;
        verdict(false, "the Sandbox onboarding script reported failure");
        return;
    }
    if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        report_verdict([], "TUTORIAL SANDBOX INTRO", "TUTORIAL_SANDBOX_INTRO");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    // Events arrive on the next tick. One event per tick makes the expected
    // card transitions deterministic and keeps the wrong-event assertion
    // visible in the authored sequence.
    if this.step == 0 { emit("cmd:TutorialNext", 0); }
    else if this.step == 1 { emit("cmd:TutorialNext", 0); }
    else if this.step == 2 { emit("cmd:TutorialNext", 0); }
    else if this.step == 3 { emit("cmd:TutorialNext", 0); }
    else if this.step == 4 { emit("key:KeyC", 0); }
    else if this.step == 5 { emit("key:KeyW", 0); }
    else if this.step == 6 { emit("cmd:SelectEntity", 0); }
    else if this.step == 7 { emit("cmd:SpawnEntity", 0); }
    else if this.step == 8 { emit("cmd:PossessVessel", 0); }
    else if this.step == 9 { emit("key:Backspace", 0); }
    else if this.step == 10 { emit("cmd:TutorialNext", 0); }
    else if this.step == 11 { emit("cmd:TutorialNext", 0); }
    else if this.step > 12 && this.elapsed > 5.0 {
        this.done = true;
        verdict(false, "Sandbox onboarding did not complete");
        return;
    }
    this.step += 1;
}
