// Rover autopilot — drives the deployed rover through the waypoint course on its
// own, and YIELDS the instant the player touches the controls (any key).
//
// Engages on the mission's `rover_deployed` event; disengages permanently on any
// `key:*` event. Driving is a control loop in on_tick (a signal task). It uses
// its OWN steering rather than the prelude's nav_to because the skid drive needs
// the right sign + a 180-degree turn-around case:
//   - skid mix is `left = drive + steer`, so +steer yaws toward the RIGHT (the
//     slower side); cross.y > 0 means the target is to the LEFT, so steer = -cross.y.
//   - when the target is BEHIND (dot < 0) the cross is ~0, so force a hard turn.
//   - ease off the throttle while turning so it pivots toward the gate instead of
//     arcing past it.

fn wp_pos(i) { world_pos(find("/LanderTest/RoverTarget" + i)) }

/// Signed steer in [-1,1] turning the rover toward `target`.
fn steer_toward(me, target) {
    let p = world_pos(me);
    let f = world_forward(me);
    if p == () || f == () { return 0.0; }
    let to = vnorm(vsub(target, p));
    let fwd = vnorm(f);
    let cy = vcross(fwd, to)[1];
    let dot = vdot(fwd, to);
    if dot < 0.0 {
        if cy >= 0.0 { -1.0 } else { 1.0 }   // target behind: hard turn toward it
    } else {
        clamp(-cy * 2.5, -1.0, 1.0)
    }
}

fn on_tick(me) {
    if this.active != true { return; }   // not deployed yet
    if this.manual == true { return; }   // player took over
    if this.i == () { this.i = 1; }
    if this.i > 3 { brake(me); return; } // course complete

    let target = wp_pos(this.i);
    if target == () { return; }          // waypoint not loaded yet

    let p = world_pos(me);
    if p != () && vlen(vsub(p, target)) <= 4.0 {
        this.i += 1;                     // reached this gate; on to the next
        return;
    }

    let steer = steer_toward(me, target);
    // Slow down while turning (low forward alignment) so it pivots, not arcs.
    let fwd = 0.7;
    let f = world_forward(me);
    if p != () && f != () {
        let dot = vdot(vnorm(f), vnorm(vsub(target, p)));
        fwd = 0.7 * clamp(0.25 + 0.75 * dot, 0.25, 1.0);
    }
    drive(me, fwd, steer);
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
