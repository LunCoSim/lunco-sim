// Cinematic director for the lunar-descent demo (scenes/luncosim/lander_cinematic.usda).
//
// ONE example that shows BOTH animation and planning as a scripted cut-scene:
//   • ANIMATION — starts the keyframed-camera transport (`play_animation()`),
//     then cuts between an authored `timeSamples` dolly (`OrbitView`) and
//     mounted cameras that RIDE the moving lander/rover.
//   • PLANNING  — after the explicit landing-transition join it deploys the
//     rover and drives the same 3-gate survey course, consuming the marker
//     Sensor events.
//
// The camera CUTS are the cinematic layer; the land→deploy→survey PHASES are
// gated on real simulation state (the three source-qualified landing events and
// waypoint Sensor events), never on hand-counted ticks. The lander flies itself
// (the onboard GNC in scenarios/lander_subsystems.rhai) — this script only
// DIRECTS the view.

fn on_event(me, evt) {
    let rover = find("/LanderTest/SkidRover");
    if evt.name != "waypoint.reached" || evt.source != rover { return; }
    if evt.value == "/LanderTest/RoverTarget1" {
        emit("waypoint_1_reached");
    } else if evt.value == "/LanderTest/RoverTarget2" {
        emit("waypoint_2_reached");
    } else if evt.value == "/LanderTest/RoverTarget3" {
        emit("waypoint_3_reached");
    }
}

fn cinematic_intro_notice(me) {
    play_animation();
    notify_kind("T-0 - powered descent from orbit.", "info");
}

fn tracking_notice(me) {
    notify_kind("Tracking powered descent.", "info");
}

fn approach_notice(me) {
    notify_kind("Final approach - onboard view.", "info");
}

fn touchdown_notice(me) {
    set_camera("PadCam");
    // Engine cutoff is already accepted by the typed GNC landing transition
    // before this join completes; the director only changes the camera and UI.
    notify_kind("Touchdown. Engine cutoff accepted.", "success");
}

fn deploy_notice(me) {
    detach_joint(find("/LanderTest/LanderRoverJoint"));
    notify("Releasing the rover...");
}

fn engage_notice(me) {
    set_camera("RoverCam");
    emit("rover_deployed");
    cmd("EngageAutopilot", #{ vessel: find("/LanderTest/SkidRover") });
    notify_kind("Rover deployed - driving the survey course.", "success");
}

fn gate_one_notice(me) {
    notify_kind("Waypoint 1 reached (2 of 3).", "success");
}

fn gate_two_notice(me) {
    notify_kind("Waypoint 2 reached (3 of 3).", "success");
}

fn survey_complete_notice(me) {
    set_camera("WideTrack");
    notify_kind("Survey complete - mission accomplished.", "success");
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`).
fn cinematic_intro() {
    seq([
        // ── Beat 1: wide heroic establishing (mounted, auto-framing) ─────────
        // The camera CUTS for the fixed-time beats (WideTrack@0 → TrackCam@6 →
        // DescentCam@12) now live as DATA on the scene's `CameraTrack`
        // (`lunco:activeCamera.timeSamples`); the camera-track sampler fires them
        // as the animation transport advances. This director only starts the
        // transport and narrates — it no longer imperatively `set_camera(...)`s
        // the timed cuts (doc 35, slice 1). Progressive mounted cameras (wide →
        // medium → onboard) all ride the descending lander and auto-aim, so each
        // cut reads as a continuous push-in / zoom toward the surface.
        once(|m| cinematic_intro_notice(m)),
        wait(6.0),
    ])
}

fn cinematic_approach() {
    seq([

        // ── Beat 2: closer external tracking shot (cut driven by CameraTrack) ─
        once(|m| tracking_notice(m)),
        wait(6.0),

        // ── Beat 3: onboard descent POV (cut driven by CameraTrack) ─────────
        once(|m| approach_notice(m)),

        // ── Beat 4: landing transition join ────────────────────────────────
        // The physical touchdown and both GNC mode transitions are independent
        // producers; release/camera policy waits for all three explicitly.
        par_all([
            wait_for_from("lander_engine_cutoff", "/LanderTest/Lander/GNC"),
            wait_for_from("lander_flight_handoff", "/LanderTest/Lander/GNC"),
            wait_for_from("lander_touchdown", "/LanderTest/Lander"),
        ]),
        once(|m| touchdown_notice(m)),
    ])
}

fn deploy_rover() {
    seq([
        wait(2.0),

        // ── Beat 5: deploy the rover ────────────────────────────────────────
        once(|m| deploy_notice(m)),
        wait(2.0),
        once(|m| engage_notice(m)),
    ])
}

fn gate_one() {
    seq([
        // ── Beat 6: planned 3-gate survey (chase cam follows) ───────────────
        wait_for("waypoint_1_reached"),
        once(|m| gate_one_notice(m)),
    ])
}

fn gate_two() {
    seq([
        wait_for("waypoint_2_reached"),
        once(|m| gate_two_notice(m)),
    ])
}

fn gate_three() {
    seq([
        wait_for("waypoint_3_reached"),

        // ── Beat 7: pull back to the wide shot ──────────────────────────────
        once(|m| survey_complete_notice(m)),
    ])
}

fn intro_and_touchdown() {
    seq([cinematic_intro(), cinematic_approach()])
}

fn deploy_and_survey() {
    seq([deploy_rover(), gate_one(), gate_two(), gate_three()])
}

fn task(me) {
    seq([intro_and_touchdown(), deploy_and_survey()])
}
