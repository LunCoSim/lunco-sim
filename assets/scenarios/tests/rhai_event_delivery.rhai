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

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.event_order = [];
    this.event_order_checked = false;
    subscribe("MISSION_COMPLETE");
    subscribe("ORDER_A");
    subscribe("ORDER_Z");

    // Deliberately enqueue the reverse of the canonical same-tick order.
    // Event delivery must use event identity, not observer arrival order.
    emit("ORDER_Z", 0);
    emit("ORDER_A", 0);
}

fn mission(me, ctx) {
    [objective("burst_received", #{
        text: "Receive the event burst through the mission event contract",
        requires_event: "mission_event_storm",
    })]
}

fn on_event(me, evt, ctx) {
    if evt.name == "ORDER_A" || evt.name == "ORDER_Z" {
        this.event_order.push(evt.name);
        if this.event_order.len() == 2 {
            this.event_order_checked = true;
            if this.event_order[0] != "ORDER_A" || this.event_order[1] != "ORDER_Z" {
                this.done = true;
                verdict(false, "same-tick scenario events were delivered in arrival order");
            }
        }
    }
    if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        if this.event_order_checked {
            verdict(true, "");
        } else {
            verdict(false, "the same-tick event ordering assertion did not complete");
        }
    }
}

// Test-only producer: a production authored scenario may use on_tick for a
// deterministic fixture, while tutorial policy remains event/state driven.
fn on_tick(me, ctx) {
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
