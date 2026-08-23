//! B3 — Slopes & Tip-Over (Basic tutorial track).
//!
//! The tip-over lesson, on the deliberately top-heavy Hauler. Two truths a driver
//! must feel: (1) a borderline grade is climbable straight up the fall-line but
//! (2) steering ACROSS it rolls a high-centre-of-mass rover. The student is
//! coached to first make it lean on purpose, use the recovery control if it rolls,
//! then climb it properly. Completion is possess → feel more than 15 degrees of
//! tilt → enter the top marker's authored waypoint trigger; recovery is coached
//! when needed but is not a completion condition. Tilt is read with `is_tipped` /
//! `tilt_deg` (prelude, off `world_rotation`).

fn on_start(me) {
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    focus_panel("sandbox_inspector");
    spotlight("panel.right_inspector", "Review the selected Hauler's authored mass and centre-of-mass properties.");
    notify_kind("Tutorial: Slopes & Tip-Over", "info");
    hint("This is the Hauler — heavy and top-heavy on purpose. Click it, then click Possess in the Inspector or Command Deck.");
}

fn b3_possessed(me) {
    hint("First, an experiment. Drive onto the grade and steer ACROSS the slope — watch the top-heavy Hauler start to roll. If it goes over, use the Recover tool to put it back on its wheels. Witness where the edge is.");
}

fn b3_edge_felt(me) {
    clear_spotlight();
    let msg = if is_tipped(find("/SlopeTest/Rover"), 15.0) {
        "Over it went — recover it explicitly. THAT is a high centre of mass."
    } else {
        "Feel that lean? A few more degrees across the fall-line and the Hauler goes over."
    };
    notify_kind(msg, "warn");
    hint("Now do it properly: point straight up the fall-line and climb to the marker with small, gentle inputs.");
}

fn b3_tilted(me) {
    let rover = find("/SlopeTest/Rover");
    rover >= 0 && tilt_deg(rover) > 15.0
}

fn b3_controlled(me) {
    let rover = find("/SlopeTest/Rover");
    rover >= 0 && controller(rover) != ()
}

fn b3_reached_top(me) {
    notify_kind("Straight up, gentle throttle — that's how a bad rover climbs a good line.", "success");
}

fn b3_autopilot_possess(me) {
    cmd("PossessVessel", #{ target: find("/SlopeTest/Rover") });
    notify_kind(`[debug] tilt at start: ${tilt_deg(find("/SlopeTest/Rover"))}°`, "info");
}

fn b3_cross_grade(me) {
    nav_to(find("/SlopeTest/Rover"), world_pos(find("/SlopeTest/Marker")), 0.55, 5.0)
}

fn b3_cross_grade_done(me) {
    tilt_deg(find("/SlopeTest/Rover")) > 15.0
}

fn b3_climb(me) {
    nav_to(find("/SlopeTest/Rover"), world_pos(find("/SlopeTest/Marker")), 0.55, 1.5)
}

fn b3_report_tip(me) {
    if is_tipped(find("/SlopeTest/Rover"), 40.0) {
        notify_kind("[debug] rover tipped on the climb", "warn");
    }
}

fn b3_brake(me) {
    brake(find("/SlopeTest/Rover"))
}

fn mission(me) {
    [
        objective("possess", #{
            text: "Click the Hauler, then click Possess in the Inspector or Command Deck",
            done: Fn("b3_controlled"),
            on_complete: Fn("b3_possessed"),
        }),
        // Key this on TILT, not on a completed roll: 15° is reachable by steering
        // across the grade, while requiring a full tip-over would punish a careful
        // driver and make the objective impossible to complete safely.
        objective("feel_the_edge", #{
            text: "Steer ACROSS the grade until you feel it lean past 15°",
            requires: ["possess"],
            done: Fn("b3_tilted"),
            dwell: 0.3,
            on_complete: Fn("b3_edge_felt"),
        }),
        objective("reach_top", #{
            text: "Now climb straight up the fall-line to the marker at the top",
            requires: ["feel_the_edge"],
            // The marker's USD-authored Trigger Sensor is the arrival authority.
            // Its overlap accounts for the rover's physical footprint; polling
            // the chassis origin with a second radius would disagree with it.
            requires_event: "enter:waypoint",
            requires_event_source: "/SlopeTest/Marker/Trigger",
            on_complete: Fn("b3_reached_top"),
        }),
    ]
}

fn on_event(me, evt) {
    if evt.name == "MISSION_COMPLETE" {
        clear_spotlight();
        hint("You've felt why a high centre of mass tips, and how to climb anyway: pick the fall-line, keep steering inputs small on a grade. Click Continue in the Tutorial complete dialog to compare the rover variants.");
        notify_kind("Lesson complete — choose Continue for Rover Variants", "success");
    }
}

// UNATTENDED autopilot — possess and climb straight to the marker. Logs tilt via
// the prelude's is_tipped/tilt_deg (which read world_rotation) so the accessor is
// exercised in CI. With a window open, a human drives.
fn task(me) {
    if !is_unattended() { return; }
    seq([
        wait(1.0),
        once(Fn("b3_autopilot_possess")),
        wait(0.5),
        // Cross the grade first so the debug run satisfies `feel_the_edge` — the
        // demo must exercise the same objective chain a student does, or CI proves
        // a path nobody takes.
        step(Fn("b3_cross_grade"), Fn("b3_cross_grade_done")),
        // The task event is scoped to this lesson's authored Trigger Sensor.
        #{ act: Fn("b3_climb"), event: "enter:waypoint", src: "/SlopeTest/Marker/Trigger" },
        once(Fn("b3_report_tip")),
        #{ act: Fn("b3_brake"), secs: 1.5 },
    ])
}
