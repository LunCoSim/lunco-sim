// Cinematic director for the lunar-descent demo (scenes/sandbox/lander_cinematic.usda).
//
// ONE example that shows BOTH animation and planning as a scripted cut-scene:
//   • ANIMATION — starts the keyframed-camera transport (`play_animation()`),
//     then cuts between an authored `timeSamples` dolly (`OrbitView`) and
//     mounted cameras that RIDE the moving lander/rover.
//   • PLANNING  — after touchdown it deploys the rover and drives the same
//     3-gate survey course, revealing gates one at a time.
//
// The camera CUTS are the cinematic layer; the land→deploy→survey PHASES are
// gated on real simulation state (`wait_for("lander_touchdown")`, arrival
// distance), never on hand-counted ticks. The lander flies itself (the onboard
// GNC in scenarios/lander_subsystems.rhai) — this script only DIRECTS the view.

/// Reveal (`on`) or hide a waypoint dome by scaling its mesh.
fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/LanderTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

/// True once the rover is within `r` metres of waypoint `n`.
fn at_wp(n, r) {
    let t = world_pos(find("/LanderTest/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/LanderTest/SkidRover"), t, r)
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`).
fn task(me) {
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
        once(|m| {
            play_animation();              // advance the timeline → the CameraTrack cuts
            show_wp(2, false);
            show_wp(3, false);
            notify_kind("T-0 - powered descent from orbit.", "info");
        }),
        wait(6.0),

        // ── Beat 2: closer external tracking shot (cut driven by CameraTrack) ─
        once(|m| {
            notify_kind("Tracking powered descent.", "info");
        }),
        wait(6.0),

        // ── Beat 3: onboard descent POV (cut driven by CameraTrack) ─────────
        once(|m| {
            notify_kind("Final approach - onboard view.", "info");
        }),

        // ── Beat 4: touchdown (gated on the lander's own event) ─────────────
        wait_for("lander_touchdown"),
        once(|m| {
            set_camera("PadCam");
            set(find("/LanderTest/Lander"), "engine_enable", 0.0);   // safe the engine
            notify_kind("Touchdown. Engine safed.", "success");
        }),
        wait(2.0),

        // ── Beat 5: deploy the rover ────────────────────────────────────────
        once(|m| {
            detach_joint(find("/LanderTest/LanderRoverJoint"));
            notify("Releasing the rover...");
        }),
        wait(2.0),
        once(|m| {
            set_camera("RoverCam");
            emit("rover_deployed");        // the rover's autopilot engages on this
            notify_kind("Rover deployed - driving the survey course.", "success");
        }),

        // ── Beat 6: planned 3-gate survey (chase cam follows) ───────────────
        wait_until(|m| at_wp(1, 5.0)),
        once(|m| {
            show_wp(1, false);
            show_wp(2, true);
            notify_kind("Waypoint 1 reached (2 of 3).", "success");
        }),
        wait_until(|m| at_wp(2, 5.0)),
        once(|m| {
            show_wp(2, false);
            show_wp(3, true);
            notify_kind("Waypoint 2 reached (3 of 3).", "success");
        }),
        wait_until(|m| at_wp(3, 5.0)),

        // ── Beat 7: pull back to the wide shot ──────────────────────────────
        once(|m| {
            show_wp(3, false);
            set_camera("WideTrack");
            notify_kind("Survey complete - mission accomplished.", "success");
        }),
    ])
}
