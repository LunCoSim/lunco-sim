// Rover autopilot — drives the deployed rover through the waypoint course on its
// own, and YIELDS the instant the player touches the controls (any key).
//
// Attached via `lunco:scriptPath` on the rover. The DRIVING is a control loop
// (continuous steering in on_tick — a signal task, not an event), but its
// LIFECYCLE is event-driven:
//   - engages on the mission's `rover_deployed` event (so it doesn't fight the
//     descent while the rover is still bolted to the lander),
//   - disengages permanently on any `key:*` event (the native input->event
//     bridge), handing control to the player.
// Waypoints are addressed by path; arrival comes from nav_to (which also brakes),
// advancing to the next gate. nav_to's radius matches the gate sensor radius, so
// the autopilot and the mission's `enter:waypoint` events stay in lock-step.

fn wp_pos(i) { world_pos(find("/LanderTest/RoverTarget" + i)) }

fn on_tick(me) {
    if this.active != true { return; }   // not deployed yet
    if this.manual == true { return; }   // player has taken over
    if this.i == () { this.i = 1; }
    if this.i > 3 { brake(me); return; } // course complete

    let target = wp_pos(this.i);
    if target == () { return; }          // waypoint not loaded yet

    // Steer toward the gate; nav_to returns true (and brakes) on arrival.
    if nav_to(me, target, 0.7, 4.0) {
        this.i += 1;
    }
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.active = true;
        this.i = 1;
    } else if evt.name.starts_with("key:") {
        // Any key hands control to the player — autopilot stands down for good.
        if this.active == true && this.manual != true {
            this.manual = true;
            brake(me);
            notify_kind("Manual control - autopilot off.", "info");
        }
    }
}
