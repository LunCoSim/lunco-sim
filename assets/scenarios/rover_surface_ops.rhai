// Mission orchestration for the on-surface rover scenario (doc 34, Decision 3).
//
// Bound from USD by the `def LuncoProgram "Mission"` child prim of
// `/LanderTest/Scenario`, whose `uniform asset lunco:program:sourceAsset` names this
// file.
// This is a DECLARATIVE task: the prelude sequencer advances it each tick using
// INTERNAL SIM TIME (`wait` = elapsed sim seconds) and STATE PREDICATES
// (`wait_until` reads the live world / model ports). No hand-rolled tick counting
// or `dt()` integration in rhai — timing and state come from the simulation.
//
// Phases: follow the lander down → wait for its `lander_touchdown` event → detach
// the rover → `rover_deployed` → reveal a 3-gate course one dome at a time.
//
// Every phase it reaches, it ANNOUNCES on the bus: `rover_deployed`,
// `waypoint_1_reached` … `waypoint_3_reached`, `course_complete`. That's the whole
// contract with anything watching — the rover's autopilot wakes on
// `rover_deployed`, the tutorial turns the rest into a student checklist. Neither
// reaches into this script's state, and this script knows about neither.

/// Reveal (`on`) or hide a waypoint dome by scaling its mesh — the same trick
/// the flame uses. Resolved by path at call time (inside a step closure).
fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/LanderTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

/// True once the rover is within `r` metres of waypoint `n`. A reliable distance
/// check that works whether the autopilot or the player drives — the rover's
/// raycast-wheel compound collider doesn't reliably trip an avian Sensor, so we
/// read positions directly instead of waiting on a zone-enter event.
fn at_wp(n, r) {
    let t = world_pos(find("/LanderTest/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/LanderTest/SkidRover"), t, r)
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`). Phases:
/// land (wait for the lander's own `lander_touchdown` event) → deploy rover →
/// drive a 3-gate course, each gate revealed only after the previous is reached.
fn task(me) {
    seq([
        once(|m| {
            // Camera rides along WITHOUT possessing: `follow` inserts a chase camera
            // (SpringArmCamera) but no ControllerLink, so the lander stays UNPOSSESSED
            // → its `piloted` port reads 0 → the internal GNC flies it. Possessing it
            // would set piloted=1 and hand control to the pilot — reserved for a human
            // (or an autopilot) deliberately taking over.
            follow(find("/LanderTest/Lander"));
            show_wp(2, false);
            show_wp(3, false);
            notify_kind("Powered descent - the GNC is flying the lander down.", "info");
        }),
        // The lander's supervisor emits `lander_touchdown` when it settles.
        wait_for("lander_touchdown"),
        once(|m| {
            notify_kind("Touchdown.", "success");
        }),
        wait(1.5),                                       // brief settle (sim seconds)
        once(|m| {
            detach_joint(find("/LanderTest/LanderRoverJoint"));
            notify("Releasing the rover...");
        }),
        wait(2.0),                                       // let the rover fall clear
        once(|m| {
            // Engage the rover's autopilot (it drives the course until the player
            // takes the wheel). The rover's own script listens for this event.
            emit("rover_deployed");
            notify_kind("Rover deployed - autopilot driving the course. Click the rover (or press F) to take over.", "success");
        }),

        // ── 3-gate course ──────────────────────────────────────────────────
        // Arrival = the rover within range of each gate (reliable distance check).
        // Each arrival hides the reached dome, reveals the next — "show the next
        // after it's done once" — and emits its own event for watchers.
        wait_until(|m| at_wp(1, 5.0)),
        once(|m| {
            show_wp(1, false);
            show_wp(2, true);
            emit("waypoint_1_reached");
            notify_kind("Waypoint 1 reached! Next dome revealed (2 of 3).", "success");
        }),
        wait_until(|m| at_wp(2, 5.0)),
        once(|m| {
            show_wp(2, false);
            show_wp(3, true);
            emit("waypoint_2_reached");
            notify_kind("Waypoint 2 reached! Final dome revealed (3 of 3).", "success");
        }),
        wait_until(|m| at_wp(3, 5.0)),
        once(|m| {
            show_wp(3, false);
            emit("waypoint_3_reached");
            emit("course_complete");
            notify_kind("All waypoints reached - course complete!", "success");
        }),
    ])
}
