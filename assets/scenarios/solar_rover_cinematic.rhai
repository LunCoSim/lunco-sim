// Solar Rover Cinematic Director
// Coordinates camera cuts and subtitles. The sun, panel tracking, and battery
// dynamics are physics — they run without any cinematic involvement.

fn on_event(me, evt) {
    let rover = find("/SolarRoverTest/SolarRover");
    if evt.name != "waypoint.reached" || evt.source != rover { return; }
    if evt.value == "/SolarRoverTest/RoverTarget1" {
        emit("waypoint_1_reached");
    } else if evt.value == "/SolarRoverTest/RoverTarget2" {
        emit("waypoint_2_reached");
    } else if evt.value == "/SolarRoverTest/RoverTarget3" {
        emit("waypoint_3_reached");
    }
}

fn soc() {
    let r = find("/SolarRoverTest/SolarRover");
    if r < 0 { return (); }
    get(r, "soc")
}

fn intro_notice(me) {
    notify_kind("Lunar Co-Simulation: Solar-Powered Rover System", "info");
}

fn deploy_notice(me) {
    emit("rover_deployed");
    cmd("EngageAutopilot", #{ vessel: find("/SolarRoverTest/SolarRover") });
    notify_kind("Autopilot engaged. Battery begins draining under load...", "info");
}

fn battery_low(me) {
    let s = soc();
    s != () && s < 0.05
}

fn battery_low_notice(me) {
    set_camera("SideCam");
    notify_kind("Emergency: Battery depleted! Starting sun-tracking solar charge.", "warn");
}

fn charging_notice(me) {
    notify_kind("The solar panel is tracking the sun. Charging...", "info");
}

fn battery_ready(me) {
    let s = soc();
    s != () && s >= 0.20
}

fn resume_notice(me) {
    set_camera("RoverCam");
    notify_kind("Battery charged to 20%. Continuing to Waypoint 1.", "success");
}

fn gate_one_notice(me) {
    notify_kind("Waypoint 1 reached. Heading to Waypoint 2.", "success");
}

fn gate_two_notice(me) {
    notify_kind("Waypoint 2 reached. Heading to Waypoint 3.", "success");
}

fn final_notice(me) {
    set_camera("WideTrack");
    notify_kind("Mission Accomplished! Co-simulation and autopilot successful.", "success");
}

fn pause_notice(me) {
    pause();
    notify_kind("Simulation paused. End of demonstration.", "info");
}

fn intro_and_deploy() {
    seq([
        // ── Beat 1: Intro ─────────
        once(|m| intro_notice(m)),
        wait(5.0),

        // ── Beat 2: Deploy ─────────
        once(|m| deploy_notice(m)),
    ])
}

fn battery_depleted() {
    seq([

        // ── Beat 3: Wait for battery depletion ─────────
        wait_until(|m| battery_low(m)),

        // ── Beat 4: Charging showcase ─────────
        // The panel is already tracking the sun (SunTracker.mo runs continuously).
        // Cut to the side camera so the panel orientation is visible.
        once(|m| battery_low_notice(m)),
    ])
}

fn battery_charging() {
    seq([
        wait(3.0),
        once(|m| charging_notice(m)),
    ])
}

fn battery_recharged() {
    seq([

        // ── Beat 5: Wait for recharge ─────────
        wait_until(|m| battery_ready(m)),

        // ── Beat 6: Resume ─────────
        once(|m| resume_notice(m)),
    ])
}

fn battery_cycle() {
    seq([battery_depleted(), battery_charging(), battery_recharged()])
}

fn gate_one() {
    seq([
        // ── Beat 7: Gate 1 ─────────
        wait_for("waypoint_1_reached"),
        once(|m| gate_one_notice(m)),
    ])
}

fn gate_two() {
    seq([

        // ── Beat 8: Gate 2 ─────────
        wait_for("waypoint_2_reached"),
        once(|m| gate_two_notice(m)),
    ])
}

fn gate_three() {
    seq([

        // ── Beat 9: Final gate ─────────
        wait_for("waypoint_3_reached"),
        once(|m| final_notice(m)),
        wait(5.0),
        once(|m| pause_notice(m))
    ])
}

fn survey() {
    seq([gate_one(), gate_two(), gate_three()])
}

fn task(me) {
    seq([intro_and_deploy(), battery_cycle(), survey()])
}
