//! Positive production regression for the native mission event projection.
//!
//! The first fixed pass emits an admitted mission event before a large burst
//! of unrelated traffic. The mission receives the required identity, while
//! serial script emissions retain their causal order and distinct RNG streams.

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
    this.repeats = [];
    this.rng_samples = [];
    subscribe("MISSION_COMPLETE");
    subscribe("ORDER_A");
    subscribe("ORDER_Z");
    subscribe("ORDER_REPEAT");

    // These serial script emissions are ordered input facts, not unordered
    // telemetry. The lexical name or payload must not reorder them.
    emit("ORDER_Z", 0);
    emit("ORDER_A", 0);
    emit("ORDER_REPEAT", 2);
    emit("ORDER_REPEAT", 1);
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
            if this.event_order[0] != "ORDER_Z" || this.event_order[1] != "ORDER_A" {
                this.done = true;
                verdict(false, "same-tick causal scenario events were reordered");
            }
        }
    }
    if evt.name == "ORDER_REPEAT" {
        this.repeats.push(evt.value);
        this.rng_samples.push(rand());
    }
    if evt.name == "MISSION_COMPLETE" && !this.done {
        this.done = true;
        if this.event_order_checked && this.repeats == [2,1] && this.rng_samples.len()==2 && this.rng_samples[0]!=this.rng_samples[1] {
            verdict(true, "");
        } else {
            verdict(false, "causal name/payload order or distinct same-name event RNG assertion failed");
        }
    }
}

// Test-only producer: a production authored scenario may use on_tick for a
// deterministic fixture, while tutorial policy remains event/state driven.
fn on_tick(me, ctx) {
    if this.done { return; }
    if this.elapsed == 0.0 {
        emit("mission_event_storm", 0);
        for i in 0..STORM_COUNT {
            emit("unrelated_mission_traffic", i);
        }
    }
    this.elapsed += dt();
    if this.elapsed > 5.0 && !this.done {
        this.done = true;
        verdict(false, "the mission did not consume the event burst");
    }
}
