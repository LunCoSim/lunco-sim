//! First Drive tutorial — orchestrator attached to /FirstDrive/Scenario.
//!
//! A declarative `mission(me)`: each objective advances when the student does
//! the real thing, not on a timer. Possession and arrival are checked against
//! the rover's live controller and distance to this lesson's named flag. The
//! engine auto-publishes the
//! objectives checklist to the tutorial HUD, so this whole lesson is ~30 lines
//! and no Rust.

fn on_start(me) {
    // The curriculum payload is mounted before this script starts. This script
    // only teaches and verifies the rover actions in that world.
    hint("Welcome to LunCoSim! Let's drive a rover on the Moon.");
    notify_kind("Tutorial: First Drive", "info");
}

fn first_drive_possessed(me) {
    hint("Now use W / A / S / D to drive to the glowing flag.");
}

fn first_drive_finished(me) {
    notify_kind("You reached the flag — nice driving!", "success");
}

fn first_drive_controlled(me) {
    let rover = find("/FirstDrive/Rover");
    rover >= 0 && controller(rover) != ()
}

fn first_drive_at_flag(me) {
    let rover = find("/FirstDrive/Rover");
    let flag = find("/FirstDrive/Flag");
    if rover < 0 || flag < 0 { return false; }
    arrived(rover, world_pos(flag), 2.5)
}

fn mission(me) {
    // Scene entity paths as LOCALS (not top-level `const`): rhai closures capture
    // enclosing locals by value, but named fns can't see module consts — so the
    // `done` closure below captures these, resolving each via find() every tick.
    [
        // Step 1 — take control. A request is not success; live controller state
        // must show that this rover is actually owned.
        objective("possess", #{
            text: "Click the rover, then click Possess in the Inspector or Command Deck.",
            done: Fn("first_drive_controlled"),
            on_complete: Fn("first_drive_possessed"),
        }),
        // Step 2 — drive to this lesson's named flag. A generic marker event is
        // not enough to prove the correct rover reached the correct target.
        objective("reach_flag", #{
            text: "Drive to the glowing flag",
            requires: ["possess"],
            done: Fn("first_drive_at_flag"),
            on_complete: Fn("first_drive_finished"),
        }),
    ]
}

fn on_event(me, evt) {
    // The engine emits MISSION_COMPLETE once every objective is done.
    if evt.name == "MISSION_COMPLETE" {
        hint("Tutorial complete! Free-roam, or pick another lesson from the Tutorials panel.");
        notify_kind("First Drive complete", "success");
    }
}

// UNATTENDED autopilot — runs ALONGSIDE the mission (a scenario may have both a
// `task` behaviour and a `mission` tracker). Gated on the AUDIENCE: with no
// window (CI, `scene_test`, a headless smoke) it possesses the rover and drives
// to the flag hands-free, auto-passing the lesson; with a window open it returns
// () so the student plays. This is the "if unattended, autopilot" pattern — the
// conditional lives in the scenario, keyed off is_unattended().
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

fn first_drive_autopilot_possess(me) {
    let rover = find("/FirstDrive/Rover");
    cmd("PossessVessel", #{ target: rover });
    notify_kind("[debug] autopilot engaged", "info");
}

fn first_drive_cruise(me) {
    nav_to(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 1.0, 8.0)
}

fn first_drive_cruise_done(me) {
    arrived(find("/FirstDrive/Rover"), world_pos(find("/FirstDrive/Flag")), 8.0)
}

fn first_drive_creep(me) {
    nav_to(find("/FirstDrive/Rover"), aim_short_of(find("/FirstDrive/Rover"), find("/FirstDrive/Flag"), 1.5), 0.15, 0.6)
}

fn first_drive_creep_done(me) {
    arrived(find("/FirstDrive/Rover"), aim_short_of(find("/FirstDrive/Rover"), find("/FirstDrive/Flag"), 1.5), 0.6)
}

fn first_drive_brake(me) {
    brake(find("/FirstDrive/Rover"))
}

fn task(me) {
    if !is_unattended() { return; }   // a human is watching: they drive
    seq([
        wait(1.0),                     // let the scene settle / rover spawn
        once(Fn("first_drive_autopilot_possess")),
        wait(0.5),
        // Phase 1: cruise toward the flag at a moderate speed until within 8 m.
        step(Fn("first_drive_cruise"), Fn("first_drive_cruise_done")),
        // Phase 2: creep SLOWLY to a point 1.5 m short of the flag (an offset in
        // front of it) and stop there — so the rover's centre parks a body-length
        // out with its nose AT the flag, instead of ramming through and settling
        // ~2.3 m back. Aiming short, not just braking, is what puts it on the flag.
        step(Fn("first_drive_creep"), Fn("first_drive_creep_done")),
        // Brake and HOLD for a couple of sim-seconds so it parks instead of coasting.
        // `act_for` re-brakes every tick while the dwell holds the step, so the
        // brake persists (a single pulse would let it roll onward).
        act_for(Fn("first_drive_brake"), 2.0),
    ])
}
