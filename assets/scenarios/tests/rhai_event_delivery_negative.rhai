//! Negative production regression for event-gated mission objectives.

fn verdict(pass, reason) {
    if pass {
        emit("RHAI_EVENT_DELIVERY_NEGATIVE", "PASS");
    } else {
        print("RHAI_EVENT_DELIVERY_NEGATIVE FAIL: " + reason);
        emit("RHAI_EVENT_DELIVERY_NEGATIVE", "FAIL");
    }
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    subscribe("MISSION_COMPLETE");
}

fn mission(me) {
    [objective("must_not_complete", #{
        text: "Do not complete without the required event",
        requires_event: "event_that_is_not_emitted",
    })]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        verdict(false, "an absent event completed the mission");
    }
}

// Test-only liveness verdict for the negative case.
fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 1.0 {
        this.done = true;
        verdict(true, "");
    }
}
