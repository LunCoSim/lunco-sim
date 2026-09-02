//! Positive production regression for the native mission event projection.
//!
//! The first fixed pass emits a burst larger than the native identity queue.
//! The mission must still complete from the retained name/source identities;
//! arbitrary event payloads must never be copied into mission state.

const STORM_COUNT = 1024;

fn verdict(pass, reason) {
    if pass {
        emit("RHAI_EVENT_DELIVERY", "PASS");
    } else {
        print("RHAI_EVENT_DELIVERY FAIL: " + reason);
        emit("RHAI_EVENT_DELIVERY", "FAIL");
    }
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    subscribe("MISSION_COMPLETE");
}

fn mission(me) {
    [objective("burst_received", #{
        text: "Receive the event burst through the mission event contract",
        requires_event: "mission_event_storm",
    })]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        verdict(true, "");
    }
}

// Test-only producer: a production authored scenario may use on_tick for a
// deterministic fixture, while tutorial policy remains event/state driven.
fn on_tick(me) {
    if this.done { return; }
    if this.elapsed == 0.0 {
        for i in 0..STORM_COUNT {
            emit("mission_event_storm", i);
        }
    }
    this.elapsed += dt();
    if this.elapsed > 5.0 && !this.done {
        this.done = true;
        verdict(false, "the mission did not consume the event burst");
    }
}
