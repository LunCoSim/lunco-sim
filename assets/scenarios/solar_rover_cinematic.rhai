// Solar Rover Cinematic Director
// Coordinates camera cuts and subtitles. The sun, panel tracking, and battery
// dynamics are physics — they run without any cinematic involvement.

fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/SolarRoverTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

fn at_wp(n, r) {
    let t = world_pos(find("/SolarRoverTest/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/SolarRoverTest/SolarRover"), t, r)
}

fn soc() {
    let r = find("/SolarRoverTest/SolarRover/PowerSubsystem");
    if r < 0 { return (); }
    get(r, "soc_out")
}

fn task(me) {
    seq([
        // ── Beat 1: Intro ─────────
        once(|m| {
            show_wp(1, true);
            show_wp(2, false);
            show_wp(3, false);
            set_camera("WideTrack");
            notify_kind("Lunar Co-Simulation: Solar-Powered Rover System", "info");
        }),
        wait(5.0),

        // ── Beat 2: Deploy ─────────
        once(|m| {
            set_camera("RoverCam");
            emit("rover_deployed");
            cmd("EngageAutopilot", #{ vessel: find("/SolarRoverTest/SolarRover") });
            notify_kind("Autopilot engaged. Battery begins draining under load...", "info");
        }),

        // ── Beat 3: Wait for battery depletion ─────────
        wait_until(|m| { let s = soc(); s != () && s < 0.05 }),

        // ── Beat 4: Charging showcase ─────────
        // The panel is already tracking the sun (SunTracker.mo runs continuously).
        // Cut to the side camera so the panel orientation is visible.
        once(|m| {
            set_camera("SideCam");
            notify_kind("Emergency: Battery depleted! Starting sun-tracking solar charge.", "warn");
        }),
        wait(3.0),
        once(|m| {
            notify_kind("The solar panel is tracking the sun. Charging...", "info");
        }),

        // ── Beat 5: Wait for recharge ─────────
        wait_until(|m| { let s = soc(); s != () && s >= 0.20 }),

        // ── Beat 6: Resume ─────────
        once(|m| {
            set_camera("RoverCam");
            notify_kind("Battery charged to 20%. Continuing to Waypoint 1.", "success");
        }),

        // ── Beat 7: Gate 1 ─────────
        wait_until(|m| at_wp(1, 4.0)),
        once(|m| {
            show_wp(1, false);
            show_wp(2, true);
            notify_kind("Waypoint 1 reached. Heading to Waypoint 2.", "success");
        }),

        // ── Beat 8: Gate 2 ─────────
        wait_until(|m| at_wp(2, 4.0)),
        once(|m| {
            show_wp(2, false);
            show_wp(3, true);
            notify_kind("Waypoint 2 reached. Heading to Waypoint 3.", "success");
        }),

        // ── Beat 9: Final gate ─────────
        wait_until(|m| at_wp(3, 4.0)),
        once(|m| {
            show_wp(3, false);
            set_camera("WideTrack");
            notify_kind("Mission Accomplished! Co-simulation and autopilot successful.", "success");
        }),
        wait(5.0),
        once(|m| {
            pause();
            notify_kind("Simulation paused. End of demonstration.", "info");
        })
    ])
}
