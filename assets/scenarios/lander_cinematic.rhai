// Cinematic director for the lunar-descent demo (scenes/luncosim/lander_cinematic.usda).
//
// ONE example that shows BOTH animation and planning as a scripted cut-scene:
//   • ANIMATION — starts the keyframed-camera transport (`play_animation()`),
//     then cuts between an authored `timeSamples` dolly (`OrbitView`) and
//     mounted cameras that RIDE the moving lander/rover.
//   • PLANNING  — after touchdown it deploys the rover and drives the same
//     3-gate survey course, consuming the marker Sensor events.
//
// The camera CUTS are the cinematic layer; the land→deploy→survey PHASES are
// gated on real simulation state (`wait_for("lander_touchdown")`, waypoint Sensor
// events), never on hand-counted ticks. The lander flies itself (the onboard
// GNC in scenarios/lander_subsystems.rhai) — this script only DIRECTS the view.

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
    set(find("/LanderTest/Lander"), "engine_enable", 0.0);
    notify_kind("Touchdown. Engine safed.", "success");
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
        once(Fn("cinematic_intro_notice")),
        wait(6.0),
    ])
}

fn cinematic_approach() {
    seq([

        // ── Beat 2: closer external tracking shot (cut driven by CameraTrack) ─
        once(Fn("tracking_notice")),
        wait(6.0),

        // ── Beat 3: onboard descent POV (cut driven by CameraTrack) ─────────
        once(Fn("approach_notice")),

        // ── Beat 4: touchdown (gated on the lander's own event) ─────────────
        wait_for("lander_touchdown"),
        once(Fn("touchdown_notice")),
    ])
}

fn deploy_rover() {
    seq([
        wait(2.0),

        // ── Beat 5: deploy the rover ────────────────────────────────────────
        once(Fn("deploy_notice")),
        wait(2.0),
        once(Fn("engage_notice")),
    ])
}

fn gate_one() {
    seq([
        // ── Beat 6: planned 3-gate survey (chase cam follows) ───────────────
        wait_for("waypoint_1_reached"),
        once(Fn("gate_one_notice")),
    ])
}

fn gate_two() {
    seq([
        wait_for("waypoint_2_reached"),
        once(Fn("gate_two_notice")),
    ])
}

fn gate_three() {
    seq([
        wait_for("waypoint_3_reached"),

        // ── Beat 7: pull back to the wide shot ──────────────────────────────
        once(Fn("survey_complete_notice")),
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
