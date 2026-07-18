//! First Drive tutorial — orchestrator attached to /FirstDrive/Scenario.
//!
//! A declarative `mission(me)`: each objective advances when the student does
//! the real thing, not on a timer. Possession is the `cmd:PossessVessel` event
//! (any command dispatch lands on the bus as `cmd:<Name>`); arrival is the
//! flag's `enter:waypoint` trigger-zone event. The engine auto-publishes the
//! objectives checklist to the tutorial HUD, so this whole lesson is ~30 lines
//! and no Rust.

fn on_start(me) {
    // One source: the tutorial IS this script, and it sets up its own environment.
    // Load the env-only scene (rover, flag, avatar); the mission below find()s its
    // entities once they spawn. The scenario runs on an independent host entity, so
    // it survives its own load_scene.
    load_scene("lunco://tutorials/sandbox/first_drive.usda");
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
// A world point `standoff` metres SHORT of the flag along the rover→flag line.
// We aim the final creep here, not at the flag itself: the rover's reported pose is
// its chassis CENTRE and the body is a couple of metres long, so driving the centre
// onto the flag pole just rams it and rocks back. Parking the centre a body-length
// short lands the nose right at the flag. Recomputed live is stable — we never get
// nearer the flag than `standoff`, so the rover→flag direction never degenerates.
fn aim_short_of(rover, flag, standoff) {
    let rp = world_pos(rover);
    let fp = world_pos(flag);
    if rp == () || fp == () { return fp; }
    let d = vnorm(vsub(fp, rp));                        // unit rover → flag
    vsub(fp, [d[0]*standoff, d[1]*standoff, d[2]*standoff])
}

fn task(me) {
    if !is_debug() { return; }        // release: no autopilot
    seq([
        wait(1.0),                     // let the scene settle / rover spawn
        once(|m| { cmd("PossessVessel", #{ target: find("/FirstDrive/Rover") });
                   notify_kind("[debug] autopilot engaged", "info"); }),
        wait(0.5),
        // Phase 1: cruise toward the flag at a moderate speed until within 8 m.
        step(|m| nav_to(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 0.6, 8.0),
             |m| arrived(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 8.0)),
        // Phase 2: creep SLOWLY to a point 1.5 m short of the flag (an offset in
        // front of it) and stop there — so the rover's centre parks a body-length
        // out with its nose AT the flag, instead of ramming through and settling
        // ~2.3 m back. Aiming short, not just braking, is what puts it on the flag.
        step(|m| nav_to(find("/FirstDrive/Rover"), aim_short_of(find("/FirstDrive/Rover"), find("/FirstDrive/Flag"), 1.5), 0.15, 0.6),
             |m| arrived(find("/FirstDrive/Rover"), aim_short_of(find("/FirstDrive/Rover"), find("/FirstDrive/Flag"), 1.5), 0.6)),
        // Brake and HOLD for a couple of sim-seconds so it parks instead of coasting.
        // A raw step map: `act` re-brakes every tick while the `secs` dwell holds the
        // step, so the brake persists (a single pulse would let it roll onward).
        #{ act: |m| brake(find("/FirstDrive/Rover")), secs: 2.0 },
    ])
}
