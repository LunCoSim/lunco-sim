//! Contract for the authored Sandbox onboarding event policy.
//!
//! This observer does not call the tutorial's commands. It emits only the
//! public event names that the real UI produces and checks the lesson's public
//! MISSION_COMPLETE surface. Camera guidance is intentionally Next-gated: the
//! input map belongs to the avatar, not to this tutorial.

fn on_start(me) {
    this.step = 0;
    this.done = false;
    this.elapsed = 0.0;
    this.bindings_exposed = input_binding("forward") != () && input_binding("look_button") != ();
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

    if !this.bindings_exposed {
        this.done = true;
        verdict(false, "the active input_bindings settings were not exposed to Rhai");
        return;
    }

    // Events arrive on the next tick. One event per tick makes the expected
    // card transitions deterministic and keeps the wrong-event assertion
    // visible in the authored sequence.
    if this.step == 0 { emit("cmd:TutorialNext", 0); }
    else if this.step == 1 { emit("cmd:TutorialNext", 0); }
    else if this.step == 2 { emit("cmd:TutorialNext", 0); }
    else if this.step == 3 { emit("cmd:SelectEntity", 0); }
    else if this.step == 4 { emit("cmd:SpawnEntity", 0); }
    else if this.step == 5 { emit("cmd:PossessVessel", 0); }
    else if this.step == 6 { emit("cmd:ReleaseVessel", 0); }
    else if this.step == 7 { emit("cmd:TutorialNext", 0); }
    else if this.step == 8 { emit("cmd:TutorialNext", 0); }
    else if this.step > 9 && this.elapsed > 5.0 {
        this.done = true;
        verdict(false, "Sandbox onboarding did not complete");
        return;
    }
    this.step += 1;
}
