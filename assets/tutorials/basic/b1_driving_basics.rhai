//! B1 — Moon Driving Basics (Basic tutorial track).
//!
//! The first lesson: take control of a rover and learn the three inputs
//! (throttle / steer / brake) and the camera, then climb one gentle ramp so the
//! feel of wheel slip and a grade is in the hands BEFORE the real borderline wall
//! of the scored traverse. Objective mission — advances on real actions, never a
//! timer (see first_drive.rhai for the pattern this follows).

fn on_start(me) {
    load_scene("lunco://tutorials/basic/driving_basics.usda");
    notify_kind("Basic 1/4 · Moon Driving Basics", "info");
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
        hint("You can drive, climb, and stop. Next: Slopes & Tip-Over — climb a borderline grade and feel exactly why a top-heavy rover rolls.");
        notify_kind("S1 complete 🎉 — on to Slopes & Tip-Over", "success");
    }
}

// DEBUG autopilot — runs alongside the mission for CI/regression (see
// first_drive.rhai). Release build → is_debug() false → a human drives.
//
// The drive is authored as pure DATA (a timeline) and lowered with
// `compile_timeline` — the same steps `RegisterTimeline` stores in the Twin and
// the BT.CPP-XML codec exports. `subject` drives the rover from the scenario host.
fn task(me) {
    if !is_debug() { return; }
    let r = "/DrivingBasics/Rover";
    let drive = [
        #{ wait: 1.0 },
        #{ possess: r },
        #{ wait: 0.5 },
        // Roll onto the flat first, then climb to the finish flag.
        #{ move_to: [-15.0, 0.0, 0.0], speed: 0.6, radius: 6.0, subject: r },
        #{ move_to_entity: "/DrivingBasics/Flag", speed: 0.6, radius: 5.0, subject: r },
        #{ brake: true, secs: 2.0, subject: r },
    ];
    seq(compile_timeline(drive))
}
