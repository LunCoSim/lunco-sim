//! B1 — Moon Driving Basics (Basic tutorial track).
//!
//! The first lesson: take control of a rover and learn the three inputs
//! (throttle / steer / brake) and the camera, then climb one gentle ramp so the
//! feel of wheel slip and a grade is in the hands BEFORE the real borderline wall
//! of the scored traverse. Objective mission — advances on real actions, never a
//! timer (see first_drive.rhai for the pattern this follows).

fn on_start(me) {
    load_scene("tutorials/basic/driving_basics.usda");
    notify_kind("Basic 1/5 · Moon Driving Basics", "info");
    hint("Welcome, driver. Click the rover (or press F) to take control.");
}

fn mission(me) {
    // Scene paths as locals so the `done` closures capture them (named fns can't
    // see script-level consts).
    let rover = "/DrivingBasics/Rover";
    let flag  = "/DrivingBasics/Flag";
    [
        // 1 — take control.
        objective("possess", #{
            text: "Click the rover (or press F) to take control",
            requires_event: "cmd:PossessVessel",
            on_complete: |m| hint("You're driving. W / S throttle forward and back, A / D steer, Space brakes. Right-drag orbits the camera, scroll zooms. Roll out onto the flat and get a feel for it, then line up on the ramp to the east."),
        }),
        // 2 — climb the ramp to the finish flag. The drive out + the climb are one
        // objective, measured by arrival (a plain distance read, robust to path).
        objective("reach_flag", #{
            text: "Drive out, then climb the ramp and stop at the finish flag",
            requires: ["possess"],
            done: |m| { distance(find(rover), find(flag)) < 6.0 },
            dwell: 0.4,
            on_complete: |m| notify_kind("Nice climb — that's the feel of a grade under wheels.", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("You can drive, climb, and stop. Next: Reading the Terrain — how to spot slope, shadow and radio-shadow before you commit a wheel.");
        notify_kind("S1 complete 🎉 — on to Reading the Terrain", "success");
    }
}

// DEBUG autopilot — runs alongside the mission for CI/regression (see
// first_drive.rhai). Release build → is_debug() false → a human drives.
fn task(me) {
    if !is_debug() { return; }
    let rover = "/DrivingBasics/Rover";
    let flag  = "/DrivingBasics/Flag";
    seq([
        wait(1.0),
        once(|m| { cmd("PossessVessel", #{ target: find(rover) });
                   notify_kind("[debug] autopilot engaged", "info"); }),
        wait(0.5),
        // Roll onto the flat first (satisfies roll_out), then climb to the flag.
        step(|m| nav_to(find(rover), [-15.0, 0.0, 0.0], 0.6, 6.0),
             |m| arrived(find(rover), [-15.0, 0.0, 0.0], 6.0)),
        step(|m| nav_to(find(rover), world_pos(find(flag)), 0.6, 5.0),
             |m| arrived(find(rover), world_pos(find(flag)), 5.0)),
        #{ act: |m| brake(find(rover)), secs: 2.0 },
    ])
}
