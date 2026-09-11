// Mission orchestration for the on-surface rover scenario (doc 34, Decision 3).
//
// Bound from USD by `LunCoProgramAPI` on the `Mission` Scope of
// `/LanderTest/Scenario`, whose `uniform asset info:sourceAsset` names this
// file.
// This is a DECLARATIVE task: the prelude sequencer advances it each tick using
// INTERNAL SIM TIME (`wait` = elapsed sim seconds) and STATE PREDICATES
// (`wait_until` reads the live world / model ports). No hand-rolled tick counting
// or `dt()` integration in rhai — timing and state come from the simulation.
//
// Phases: follow the lander down → wait for the three explicit landing
// transition events → detach the rover → `rover_deployed` → consume the route
// point Sensor events. The transition waits run concurrently because the
// physical touchdown predicate and the target-qualified GNC handoff are
// independent producers and may become true on different simulation steps.
//
// Every phase it reaches, it ANNOUNCES on the bus: `rover_deployed`,
// `waypoint_1_reached` … `waypoint_3_reached`, `course_complete`. That's the whole
// contract with anything watching — the scene route program wakes on
// `rover_deployed`, the tutorial turns the rest into a student checklist. Neither
// reaches into this script's state, and this script knows about neither.

// The route-point's invisible ground-anchored Trigger is its overlap Sensor.
// The route program consumes its generic `enter:<zone>` event and emits an
// ordered `route_point_reached` index; this script translates that event into
// mission milestones.
const LANDER = "/LanderTest/Lander";
const DOCK = "/LanderTest/LanderRoverJoint";

fn on_start(me) {
    this.lander_settled = false;
    this.release_latched = false;
    this.rover_detached = false;
}

fn on_event(me, evt) {
    if evt.name == "cmd:SetPorts" {
        let transition = dock_release_edge(
            find(LANDER), this.release_latched, this.lander_settled, this.rover_detached);
        this.release_latched = transition.held;
        if transition.fire {
            let result = cmd("DetachJoint", #{
                target: find(DOCK),
                intent: "Interactive",
            });
            if !result.ok {
                print("[lander] dock release failed: " + result.error);
                notify_kind("Dock release failed: " + result.error, "error");
            } else {
                this.rover_detached = true;
                notify("Releasing the rover...");
            }
        }
        return;
    }
    if evt.name == "lander_touchdown" && evt.source == find(LANDER) {
        this.lander_settled = true;
        return;
    }
    let route = find("/LanderTest/Route");
    if evt.name != "route_point_reached" || evt.source != route { return; }
    let index = evt.value;
    if index == () { return; }
    let milestones = [
        "waypoint_1_reached",
        "waypoint_2_reached",
        "waypoint_3_reached",
    ];
    if index < milestones.len() {
        emit(milestones[index]);
    }
    if index == milestones.len() - 1 {
        emit("course_complete");
    }
}

fn begin_descent(me) {
    follow(find("/LanderTest/Lander"));
    notify_kind("Powered descent - the GNC is flying the lander down.", "info");
}

fn touchdown_notice(me) {
    notify_kind("Touchdown.", "success");
}

fn release_notice(me) {
    let joint = find(DOCK);
    if joint < 0 { return; }
    let result = cmd("DetachJoint", #{ target: joint });
    if !result.ok {
        print("[lander] mission dock release failed: " + result.error);
        notify_kind("Dock release failed: " + result.error, "error");
        return;
    }
    notify("Releasing the rover...");
}

fn engage_notice(me) {
    emit("rover_deployed");
    emit("route_start");
    notify_kind("Rover deployed - the authored route program is driving the course. Click the rover to take over.", "success");
}

fn gate_one_notice(me) {
    notify_kind("Waypoint 1 reached! Continue to marker 2 (2 of 3).", "success");
}

fn gate_two_notice(me) {
    notify_kind("Waypoint 2 reached! Continue to marker 3 (3 of 3).", "success");
}

fn gate_three_notice(me) {
    notify_kind("All waypoints reached - course complete!", "success");
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`). Phases:
/// land (join the lander's three explicit transition events) → deploy rover →
/// drive a 3-gate course, with each arrival reported by its marker Sensor.
fn descent() {
    seq([
        once(|m| begin_descent(m)),
        // These are independent latched projections. `par_all` makes the
        // mission order-independent without polling or duplicating latch state.
        par_all([
            wait_for_from("lander_engine_cutoff", "/LanderTest/Lander/GNC"),
            wait_for_from("lander_flight_handoff", "/LanderTest/Lander/GNC"),
            wait_for_from("lander_touchdown", "/LanderTest/Lander"),
        ]),
        once(|m| touchdown_notice(m)),
    ])
}

fn release_rover() {
    seq([
        wait(1.5),                                       // brief settle (sim seconds)
        once(|m| release_notice(m)),
        wait(2.0),                                       // let the rover fall clear
    ])
}

fn engage_rover() {
    seq([
        once(|m| engage_notice(m)),
    ])
}

fn gate_one() {
    seq([
        // ── 3-gate course ──────────────────────────────────────────────────
        // Arrival is the shared marker Sensor event. The USD-backed waypoint
        // projection tints only the marker that actually emitted this event.
        wait_for("waypoint_1_reached"),
        once(|m| gate_one_notice(m)),
    ])
}

fn gate_two() {
    seq([
        wait_for("waypoint_2_reached"),
        once(|m| gate_two_notice(m)),
    ])
}

fn gate_three() {
    seq([
        wait_for("waypoint_3_reached"),
        once(|m| gate_three_notice(m)),
    ])
}

fn descent_and_deploy() {
    seq([descent(), release_rover(), engage_rover()])
}

fn survey_course() {
    seq([gate_one(), gate_two(), gate_three()])
}

fn task(me) {
    seq([descent_and_deploy(), survey_course()])
}
