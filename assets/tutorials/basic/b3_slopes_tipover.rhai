//! B3 — Slopes & Tip-Over (Basic tutorial track).
//!
//! The low-grip Hauler lets learners explore two effects: a straight climb can
//! follow a grade, while a cross-slope turn can increase rollover risk. Completion
//! checks a 15-degree lean and arrival at the top marker; a full rollover and
//! recovery are not required. Tilt is read with `tilt_deg` (prelude, from
//! `world_rotation`).

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/SlopeTest/Rover");
    #{ modelica_entities: [rover],
       entity_reads: vessel_descendants(find_path("/SlopeTest")),
       entity_writes: [rover], query_reads: [], required_inputs: [] }
}
// Teaching guidance follows the learner across workbench perspectives.
hud_perspectives([]);

fn on_start(me, ctx) {
    // The optional world is selected by the catalog and mounted by the generic
    // launcher before this runs.
    focus_panel("sandbox_inspector");
    spotlight("panel.sandbox_inspector", "Select Rover at the base of the slope, then expand RockerL in Entities.");
    notify_kind("Tutorial: Slopes & Tip-Over", "info");
    hint("In Entities, select Rover at the base of the slope; it uses the Hauler profile. Expand RockerL and inspect Wheel_FL's tire selection and Motor_FL's stall torque. Reselect the Rover, then click Possess. The rover profiles share a chassis and centre of mass; Hauler has lower-grip tires than Standard.");
}

fn b3_possessed(me) {
    hint("Drive onto the grade and watch the Hauler lean. Try a slow cross-slope turn to see how its grip affects the response. If it tips, click Recover to put it back on its wheels.");
}

fn b3_edge_felt(me) {
    clear_spotlight();
    notify_kind("The Hauler leaned past 15°. Recover it if it has rolled, then continue.", "warn");
    hint("Now point straight up the fall-line. Use small inputs and climb to the marker.");
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
    cmd("AcquireControl", #{ target: find("/SlopeTest/Rover") });
    notify_kind(`[debug] tilt at start: ${tilt_deg(find("/SlopeTest/Rover"))}°`, "info");
}

fn b3_approach_grade(me) {
    nav_to(find("/SlopeTest/Rover"), world_pos(find("/SlopeTest/Marker")), 0.55, 5.0)
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

fn mission(me, ctx) {
    [
        objective("possess", #{
            text: "Select the Hauler and click Possess in Inspector or Command Deck",
            done: Fn("b3_controlled"),
            on_complete: Fn("b3_possessed"),
        }),
        // Key this on observed lean rather than a completed roll, so a careful
        // driver can complete the lesson without fully rolling the rover.
        objective("feel_the_edge", #{
            text: "Drive onto the grade and observe the Hauler lean past 15°",
            requires: ["possess"],
            done: Fn("b3_tilted"),
            dwell: 0.3,
            on_complete: Fn("b3_edge_felt"),
        }),
        objective("reach_top", #{
            text: "Climb straight up the fall-line to the top marker",
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

fn on_event(me, evt, ctx) {
    if tutorials::navigation(evt, ctx) { return; }
    if evt.name == "MISSION_COMPLETE" {
        tutorials::finish(ctx, "The Hauler leaned and you reached the top marker.");
        clear_spotlight();
        notify_kind("Lesson complete — next: Rover Variants", "success");
    }
}

// UNATTENDED autopilot — possess and climb straight to the marker. Logs tilt via
// the prelude's is_tipped/tilt_deg (which read world_rotation) so the accessor is
// exercised in CI. With a window open, a human drives.
fn task(me, ctx) {
    if !is_unattended() { return; }
    seq([
        wait(1.0),
        once(|m| b3_autopilot_possess(m)),
        wait(0.5),
        // The unattended driver approaches the marker and meets the same tilt
        // threshold before arrival. It does not exercise the optional cross-slope
        // steering maneuver suggested to learners.
        step(|m| b3_approach_grade(m), |m| b3_tilted(m)),
        // The task event is scoped to this lesson's authored Trigger Sensor.
        act_until_event(
            |m| b3_climb(m),
            "enter:waypoint",
            "/SlopeTest/Marker/Trigger",
        ),
        once(|m| b3_report_tip(m)),
        act_for(|m| b3_brake(m), 1.5),
    ])
}
