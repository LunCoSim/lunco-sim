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
// Phases: follow the lander down → wait for its `lander_touchdown` event → detach
// the rover → `rover_deployed` → consume the marker Sensor's waypoint events.
//
// Every phase it reaches, it ANNOUNCES on the bus: `rover_deployed`,
// `waypoint_1_reached` … `waypoint_3_reached`, `course_complete`. That's the whole
// contract with anything watching — the rover's autopilot wakes on
// `rover_deployed`, the tutorial turns the rest into a student checklist. Neither
// reaches into this script's state, and this script knows about neither.

// The marker's visible Sphere is also its overlap Sensor. The runtime emits
// `waypoint.reached` with the composed marker path as its value; mission policy
// translates that event into the named course events.
fn on_event(me, evt) {
    let rover = find("/LanderTest/SkidRover");
    if evt.name != "waypoint.reached" || evt.source != rover { return; }
    if evt.value == "/LanderTest/RoverTarget1" {
        emit("waypoint_1_reached");
    } else if evt.value == "/LanderTest/RoverTarget2" {
        emit("waypoint_2_reached");
    } else if evt.value == "/LanderTest/RoverTarget3" {
        emit("waypoint_3_reached");
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
    detach_joint(find("/LanderTest/LanderRoverJoint"));
    notify("Releasing the rover...");
}

fn engage_notice(me) {
    emit("rover_deployed");
    cmd("EngageAutopilot", #{ vessel: find("/LanderTest/SkidRover") });
    notify_kind("Rover deployed - autopilot driving the course. Click the rover (or press F) to take over.", "success");
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
/// land (wait for the lander's own `lander_touchdown` event) → deploy rover →
/// drive a 3-gate course, with each arrival reported by its marker Sensor.
fn descent() {
    seq([
        once(Fn("begin_descent")),
        // The lander's supervisor emits `lander_touchdown` when it settles.
        wait_for("lander_touchdown"),
        once(Fn("touchdown_notice")),
    ])
}

fn release_rover() {
    seq([
        wait(1.5),                                       // brief settle (sim seconds)
        once(Fn("release_notice")),
        wait(2.0),                                       // let the rover fall clear
    ])
}

fn engage_rover() {
    seq([
        once(Fn("engage_notice")),
    ])
}

fn gate_one() {
    seq([
        // ── 3-gate course ──────────────────────────────────────────────────
        // Arrival is the shared marker Sensor event. The USD-backed waypoint
        // projection tints only the marker that actually emitted this event.
        wait_for("waypoint_1_reached"),
        once(Fn("gate_one_notice")),
    ])
}

fn gate_two() {
    seq([
        wait_for("waypoint_2_reached"),
        once(Fn("gate_two_notice")),
    ])
}

fn gate_three() {
    seq([
        wait_for("waypoint_3_reached"),
        once(Fn("gate_three_notice")),
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
