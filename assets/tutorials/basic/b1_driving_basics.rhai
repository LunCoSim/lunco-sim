//! B1 — Moon Driving Basics (Basic tutorial track).
//!
//! The first lesson: take control of a rover and learn the three inputs
//! (throttle / steer / brake) and the camera, then climb one gentle ramp so the
//! feel of wheel slip and a grade is in the hands BEFORE the real borderline wall
//! of the scored traverse. Objective mission — advances on real actions, never a
//! timer (see first_drive.rhai for the pattern this follows).

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    // No ordinal: position is the `lunco:tutorial:next` chain in the curriculum
    // layer alone. Ordinals duplicated into scripts are how this track ended up
    // titled "2 ·" while toasting "S3".
    spotlight("panel.command_deck", "Use the Command Deck to verify who is driving before you move.");
    notify_kind("Tutorial: Moon Driving Basics", "info");
    hint("Welcome, driver. Click the rover (or press F) to take control.");
}

fn b1_driving_started(me) {
    clear_spotlight();
    spotlight("panel.telemetry_browser", "Read speed and wheel signals here while you drive.");
    hint("You're driving. W / S throttle forward and back, A / D steer, Space brakes. Read the telemetry while you roll onto the ramp.");
}

fn b1_finished(me) {
    clear_spotlight();
    notify_kind("Nice climb — that's the feel of a grade under wheels.", "success");
}

fn mission(me) {
    [
        // 1 — take control.
        objective("possess", #{
            text: "Click the rover (or press F) to take control",
            requires_event: "cmd:PossessVessel",
            on_complete: Fn("b1_driving_started"),
        }),
        // 2 — climb the ramp to the finish flag. The marker's shared Sensor
        // supplies the authoritative arrival event.
        objective("reach_flag", #{
            text: "Drive out, then climb the ramp and stop at the finish flag",
            requires: ["possess"],
            requires_event: "enter:waypoint",
            on_complete: Fn("b1_finished"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("You can drive, climb, stop, and read the result. Next: Slopes & Tip-Over — feel why a top-heavy rover rolls.");
        notify_kind("Lesson complete 🎉 — on to Slopes & Tip-Over", "success");
    }
}

// UNATTENDED autopilot — runs alongside the mission for CI/regression (see
// first_drive.rhai). A windowed session has a student in it, so they drive.
//
// The drive is authored as pure DATA (a timeline) and lowered with
// `compile_timeline` — the same steps `RegisterTimeline` stores in the Twin and
// the BT.CPP-XML codec exports. `subject` drives the rover from the scenario host.
fn task(me) {
    if !is_unattended() { return; }
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
