// Mission orchestration for the on-surface rover scenario (doc 34, Decision 3).
//
// Referenced from USD by `lunco:scriptPath` on the `/LanderTest/Scenario` prim.
// This is a DECLARATIVE task: the prelude sequencer advances it each tick using
// INTERNAL SIM TIME (`wait` = elapsed sim seconds) and STATE PREDICATES
// (`wait_until` reads the live world / model ports). No hand-rolled tick counting
// or `dt()` integration in rhai — timing and state come from the simulation.
//
// Phases: possess lander → wait for the lander's `lander_touchdown` event →
// disarm the engine → detach the rover → drive a revealed-one-at-a-time course.
// Arrivals are EVENTS (the lander's touchdown, the gates' sensor enters), not
// per-tick polls — the scenario only `wait_for`s.

/// Reveal (`on`) or hide a waypoint dome by scaling its mesh — the same trick
/// the flame uses. Resolved by path at call time (inside a step closure).
fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/LanderTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

/// The asset-relative path of a waypoint's Sensor — the EMITTER of its
/// `enter:waypoint` event. Passed to `wait_for_from` as a string and resolved
/// lazily, so all three gates can share the name "waypoint" yet be told apart by
/// source.
fn wp_zone(n) { "/LanderTest/RoverTarget" + n + "/Zone" }

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`). Phases:
/// land (wait for the lander's own `lander_touchdown` event) → deploy rover →
/// drive a 3-gate course, each gate revealed only after the previous is reached.
fn task(me) {
    seq([
        once(|m| {
            possess(find("/LanderTest/Lander"));
            // Course starts with only the first dome showing.
            show_wp(2, false);
            show_wp(3, false);
            notify_kind("Powered descent - the GNC is flying the lander down.", "info");
        }),
        // The lander's OWN script emits `lander_touchdown` when it settles — wait
        // for that event instead of polling world state here.
        wait_for("lander_touchdown"),
        // Disarm the GNC via the PORT path (`set`) — clobber-safe through
        // SimComponent.inputs. (`set_input` is the API, not a rhai verb.)
        once(|m| {
            set(find("/LanderTest/Lander"), "engine_enable", 0.0);
            notify_kind("Touchdown. Engine safed.", "success");
        }),
        wait(1.5),                                       // brief settle (sim seconds)
        once(|m| {
            detach_joint(find("/LanderTest/LanderRoverJoint"));
            notify("Releasing the rover...");
        }),
        wait(2.0),                                       // let the rover fall clear
        once(|m| {
            possess(find("/LanderTest/SkidRover"));
            // Engage the rover's autopilot (it drives the course until a key is
            // pressed). The rover's own script listens for this event.
            emit("rover_deployed");
            notify_kind("Rover deployed - autopilot driving the course. Press any key to take over.", "success");
        }),

        // ── 3-gate course ──────────────────────────────────────────────────
        // All gates fire the SAME event name ("enter:waypoint"); they're told
        // apart by the EMITTING sensor's source (wait_for_from). Each arrival
        // hides the reached dome and reveals the next — "show the next after it's
        // done once". Detection is the avian Sensor, not a distance poll.
        wait_for_from("enter:waypoint", wp_zone(1)),
        once(|m| {
            show_wp(1, false);
            show_wp(2, true);
            notify_kind("Waypoint 1 reached! Next dome revealed (2 of 3).", "success");
        }),
        wait_for_from("enter:waypoint", wp_zone(2)),
        once(|m| {
            show_wp(2, false);
            show_wp(3, true);
            notify_kind("Waypoint 2 reached! Final dome revealed (3 of 3).", "success");
        }),
        wait_for_from("enter:waypoint", wp_zone(3)),
        once(|m| {
            show_wp(3, false);
            notify_kind("All waypoints reached - mission complete!", "success");
        }),
    ])
}
