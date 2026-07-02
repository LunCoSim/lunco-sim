//! First Drive tutorial — orchestrator attached to /FirstDrive/Scenario.
//!
//! A declarative `mission(me)`: each objective advances when the student does
//! the real thing, not on a timer. Possession is the `cmd:PossessVessel` event
//! (any command dispatch lands on the bus as `cmd:<Name>`); arrival is the
//! flag's `enter:waypoint` trigger-zone event. The engine auto-publishes the
//! objectives checklist to the tutorial HUD, so this whole lesson is ~30 lines
//! and no Rust.

fn on_start(me) {
    hint("Welcome to LunCoSim! Let's drive a rover on the Moon.");
    notify_kind("Tutorial: First Drive", "info");
}

fn mission(me) {
    // Scene entity paths as LOCALS (not top-level `const`): rhai closures capture
    // enclosing locals by value, but named fns can't see module consts — so the
    // `done` closure below captures these, resolving each via find() every tick.
    let rover = "/FirstDrive/Rover";
    let flag  = "/FirstDrive/Flag";
    [
        // Step 1 — take control. Completes when a PossessVessel command fires,
        // however the student triggers it (click the rover or press F). Works for
        // the UI path now that native possession is mirrored onto the cmd:* bus.
        objective("possess", #{
            text: "Click the rover (or press F) to take control",
            requires_event: "cmd:PossessVessel",
            on_complete: |m| hint("Now use W / A / S / D to drive to the glowing flag."),
        }),
        // Step 2 — drive to the flag. Gated on step 1, then completes on distance
        // (robust: a plain world-position read, no dependence on a trigger volume).
        // dwell 0.4s so a fly-through blip doesn't count.
        objective("reach_flag", #{
            text: "Drive to the glowing flag",
            requires: ["possess"],
            done: |m| { let d = distance(find(rover), find(flag)); d >= 0.0 && d < 6.0 },
            dwell: 0.4,
            on_complete: |m| notify_kind("You reached the flag — nice driving!", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        hint("Tutorial complete! Free-roam, or pick another lesson from the Tutorials panel.");
        notify_kind("First Drive complete 🎉", "success");
    }
}

// DEBUG autopilot — runs ALONGSIDE the mission (a scenario may have both a `task`
// behaviour and a `mission` tracker). Gated on the scenario environment: in a
// debug build (or LUNCO_SCENARIO_DEBUG=1) it possesses the rover and drives to the
// flag hands-free, auto-passing the lesson (great for CI/regression); in release
// it returns () so a human plays. This is the "if DEBUG, autopilot" pattern —
// conditional behaviour lives in the scenario, keyed off is_debug().
fn task(me) {
    if !is_debug() { return; }        // release: no autopilot
    seq([
        wait(1.0),                     // let the scene settle / rover spawn
        once(|m| { cmd("PossessVessel", #{ target: find("/FirstDrive/Rover") });
                   notify_kind("[debug] autopilot engaged", "info"); }),
        wait(0.5),
        step(|m| nav_to(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 1.0, 4.0),
             |m| arrived(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 4.0)),
        once(|m| brake(find("/FirstDrive/Rover"))),
    ])
}
