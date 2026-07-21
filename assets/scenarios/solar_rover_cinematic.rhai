// Solar Rover Cinematic Director
// Coordinates camera cuts, sun azimuth animation, and subtitles/notifications.

fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/SolarRoverTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

fn at_wp(n, r) {
    let t = world_pos(find("/SolarRoverTest/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/SolarRoverTest/SolarRover"), t, r)
}

fn task(me) {
    seq([
        // ── Beat 1: Intro / Setup ─────────
        once(|m| {
            show_wp(1, true);
            show_wp(2, false);
            show_wp(3, false);
            set_camera("WideTrack");
            notify_kind("Lunar Co-Simulation: Solar-Powered Rover System", "info");
        }),
        wait(5.0),

        // ── Beat 2: Deploy Autopilot ─────────
        once(|m| {
            set_camera("RoverCam");
            emit("rover_deployed");
            notify_kind("Autopilot engaged. Battery begins draining under load...", "info");
        }),

        // ── Beat 3: Wait until battery is depleted ─────────
        wait_until(|m| {
            // SOC is a port on the power PROGRAM prim, not on the rover body.
            let r = find("/SolarRoverTest/SolarRover/PowerSubsystem");
            if r < 0 { return false; }
            let soc = get(r, "soc_out");
            soc != () && soc < 0.05
        }),

        // ── Beat 4: Halting & solar tracking demo ─────────
        once(|m| {
            set_camera("SideCam");
            notify_kind("Emergency: Battery depleted! Starting sun-tracking solar charge.", "warn");
            
            // Begin animating the sun to showcase real-time tracking
            this.rotate_sun = true;
            this.sun_yaw_val = 0.7; // Start sun yaw (rad)
        }),
        wait(3.0),
        once(|m| {
            notify_kind("Look closely: the solar panel rotates in real-time to track the sun.", "info");
        }),

        // ── Beat 5: Wait until battery is recharged ─────────
        wait_until(|m| {
            // SOC is a port on the power PROGRAM prim, not on the rover body.
            let r = find("/SolarRoverTest/SolarRover/PowerSubsystem");
            if r < 0 { return false; }
            let soc = get(r, "soc_out");
            soc != () && soc >= 0.20
        }),

        // ── Beat 6: Resuming driving ─────────
        once(|m| {
            this.rotate_sun = false; // Stop rotating the sun
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

        // ── Beat 9: Final Gate & Complete ─────────
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

// Keep the sun rotating during the charging phase to show off tracking equations
fn on_tick(me) {
    if this.rotate_sun == true {
        if this.sun_yaw_val == () { this.sun_yaw_val = 0.7; }
        this.sun_yaw_val += 0.008; // Rotate sun azimuth
        cmd("SetEnvironmentLight", #{ sun_yaw: this.sun_yaw_val });
    }
}
