// Solar Rover Cinematic Director
// Coordinates camera cuts, sun azimuth animation, and subtitles/notifications.
//
// All logic is in the behaviour tree (`task`). No `on_tick` — the sun animation
// runs inside BT nodes (`step` with `act` + `done`) so it only executes during
// the appropriate mission phase, and stops when the phase completes.

fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/SolarRoverTest/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

fn at_wp(n, r) {
    let t = world_pos(find("/SolarRoverTest/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/SolarRoverTest/SolarRover"), t, r)
}

/// The engine auto-runs a `task(me)` if defined (prelude `__init_task`).
fn task(me) {
    seq([
        // ── Beat 1: Intro / Setup ─────────
        once(|m| {
            show_wp(1, true);
            show_wp(2, false);
            show_wp(3, false);
            set_camera("WideTrack");
            notify_kind("Lunar Co-Simulation: Solar-Powered Rover System", "info");
            // Seed sun yaw for the animation that follows.
            this.sun_yaw_val = 0.3;
        }),
        wait(5.0),

        // ── Beat 2: Deploy Autopilot + slow sun drift ─────────
        // The sun drifts slowly so the panel visibly tracks during the drive.
        // `step(act, done)` runs `act` each tick until `done` returns true.
        once(|m| {
            set_camera("RoverCam");
            emit("rover_deployed");
            notify_kind("Autopilot engaged. Battery begins draining under load...", "info");
        }),

        // ── Beat 3: Drive phase — slow sun drift while waiting for battery depletion ──
        // The sun drifts gently (0.002 rad/tick ≈ 7 deg/s) so the panel is visibly
        // tracking the whole time, and we wait until the battery drops below 5%.
        step(
            |m| {
                // Slow sun drift during driving
                this.sun_yaw_val += 0.002;
                cmd("SetEnvironmentLight", #{ sun_yaw: this.sun_yaw_val });
            },
            |m| {
                // Done when battery is depleted
                let r = find("/SolarRoverTest/SolarRover/PowerSubsystem");
                if r < 0 { return false; }
                let soc = get(r, "soc_out");
                soc != () && soc < 0.05
            }
        ),

        // ── Beat 4: Halting & solar tracking showcase ─────────
        once(|m| {
            set_camera("SideCam");
            notify_kind("Emergency: Battery depleted! Starting sun-tracking solar charge.", "warn");
        }),
        wait(3.0),
        once(|m| {
            notify_kind("Look closely: the solar panel rotates in real-time to track the sun.", "info");
        }),

        // ── Beat 5: Fast sun sweep while charging ─────────
        // Faster sweep (0.008 rad/tick) to show off tracking; wait until charged.
        step(
            |m| {
                this.sun_yaw_val += 0.008;
                cmd("SetEnvironmentLight", #{ sun_yaw: this.sun_yaw_val });
            },
            |m| {
                let r = find("/SolarRoverTest/SolarRover/PowerSubsystem");
                if r < 0 { return false; }
                let soc = get(r, "soc_out");
                soc != () && soc >= 0.20
            }
        ),

        // ── Beat 6: Resuming driving ─────────
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
