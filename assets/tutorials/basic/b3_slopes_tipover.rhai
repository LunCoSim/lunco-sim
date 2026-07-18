//! B3 — Slopes & Tip-Over (Basic tutorial track).
//!
//! The tip-over lesson, on the deliberately top-heavy Hauler. Two truths a driver
//! must feel: (1) a borderline grade is climbable straight up the fall-line but
//! (2) steering ACROSS it rolls a high-centre-of-mass rover. The student is
//! coached to first make it tip on purpose (auto-righting recovers), then climb
//! it properly. Completion is reliable (possess → reach the top marker); the tilt
//! itself is read with `is_tipped` / `tilt_deg` (prelude, off `world_rotation`).

fn on_start(me) {
    load_scene("lunco://tutorials/basic/slope_test.usda");
    notify_kind("Basic 2/4 · Slopes & Tip-Over", "info");
    hint("This is the Hauler — heavy and top-heavy on purpose. Click it (or press F) to take control.");
}

fn mission(me) {
    let rover  = "/SlopeTest/Rover";
    let marker = "/SlopeTest/Marker";
    [
        objective("possess", #{
            text: "Take control of the Hauler",
            requires_event: "cmd:PossessVessel",
            on_complete: |m| hint("First, an experiment. Drive onto the grade and steer ACROSS the slope — watch the top-heavy Hauler start to roll. If it goes over, the auto-righting sets you back on your wheels. Feel where the edge is."),
        }),
        objective("reach_top", #{
            text: "Now climb straight up the fall-line to the marker at the top",
            requires: ["possess"],
            done: |m| { distance(find(rover), find(marker)) < 6.0 },
            dwell: 0.4,
            on_complete: |m| notify_kind("Straight up, gentle throttle — that's how a bad rover climbs a good line.", "success"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        hint("You've felt why a high centre of mass tips, and how to climb anyway: pick the fall-line, keep steering inputs small on a grade. Next: drive all three rovers back to back and feel the difference.");
        notify_kind("S3 complete 🎉 — on to Rover Variants", "success");
    }
}

// DEBUG autopilot — possess and climb straight to the marker. Logs tilt via the
// prelude's is_tipped/tilt_deg (which read world_rotation) so the accessor is
// exercised in CI. Release build → a human drives.
fn task(me) {
    if !is_debug() { return; }
    let rover  = "/SlopeTest/Rover";
    let marker = "/SlopeTest/Marker";
    seq([
        wait(1.0),
        once(|m| { cmd("PossessVessel", #{ target: find(rover) });
                   notify_kind(`[debug] tilt at start: ${tilt_deg(find(rover))}°`, "info"); }),
        wait(0.5),
        step(|m| nav_to(find(rover), world_pos(find(marker)), 0.55, 5.0),
             |m| arrived(find(rover), world_pos(find(marker)), 5.0)),
        once(|m| { if is_tipped(find(rover), 40.0) { notify_kind("[debug] rover tipped on the climb", "warn"); } }),
        #{ act: |m| brake(find(rover)), secs: 1.5 },
    ])
}
