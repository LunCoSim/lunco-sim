// Mission orchestration for the on-surface rover scenario (doc 34, Decision 3).
//
// Referenced from USD by `lunco:scriptPath` on the `/LanderTest/Scenario` prim.
// This is a DECLARATIVE task: the prelude sequencer advances it each tick using
// INTERNAL SIM TIME (`wait` = elapsed sim seconds) and STATE PREDICATES
// (`wait_until` reads the live world / model ports). No hand-rolled tick counting
// or `dt()` integration in rhai — timing and state come from the simulation.
//
// Phases: possess the lander → the GNC PID flies the descent → settle on legs →
// disarm the engine → detach the rover → hand control to the player.

/// True once the lander has settled on its legs: low above ground AND near-zero
/// vertical rate. Pure read of OUTSIDE DATA (world position + Avian velocity) —
/// holds no integrated state of its own.
fn lander_settled(lander) {
    let pos = world_pos(lander);
    if pos == () { return false; }
    let v = velocity(lander);
    let vy = if v == () { 0.0 } else { v[1] };
    pos[1] < 3.4 && vy > -0.1 && vy < 0.1
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`), so no
/// on_start / on_tick wiring is needed. Entities are resolved by path at step
/// time (inside the closures), not at build time. Each phase posts an on-screen
/// `notify(...)` so the player sees the mission advance.
fn task(me) {
    seq([
        once(|m| {
            possess(find("/LanderTest/Lander"));
            notify_kind("Powered descent — the GNC is flying the lander down.", "info");
        }),
        // PID flies the descent; suspend until the body is settled on its legs.
        wait_until(|m| lander_settled(find("/LanderTest/Lander"))),
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
            notify_kind("Rover deployed - you have control. Drive to the green dome.", "success");
            print("[Scenario] Rover deployed - control to player.");
        }),
        // First player task: reach the waypoint. Detection is EVENT-DRIVEN —
        // avian fires `enter:waypoint` when the rover crosses the marker's sensor
        // volume (fast, no per-tick distance poll). The processing — what arrival
        // MEANS — stays here in rhai.
        wait_for("enter:waypoint"),
        once(|m| notify_kind("Waypoint reached! Next: photograph the rock.", "success")),
    ])
}
