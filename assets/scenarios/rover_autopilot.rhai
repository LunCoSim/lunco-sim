// Rover autopilot — drives the deployed rover through the waypoint course on its
// own, and YIELDS the instant the player asks for the wheel.
//
// SCENE-AGNOSTIC: discovers waypoints via `parent(me)` (the scene root), so the
// same script works in lander_ops, solar_rover_demo, or any scene that places
// `RoverTarget1..N` siblings next to the rover.
//
// Engages on the mission's `rover_deployed` event; disengages permanently the first
// time a HUMAN takes the rover. There are two ways to ask, and both go through
// possession — the one source of truth for "who drives this":
//   • Possess the rover directly (click it / press F). `on_tick` sees a human owner
//     and stands down. Nothing is possessed on the player's behalf.
//   • Press a drive key with nothing else possessed. The autopilot possesses the
//     rover FOR you (the convenience path) and stands down.
// A keypress while the player possesses ANOTHER vessel — flying the lander with
// W/A/S/D, say — is that vessel's input, not a request for the rover. The autopilot
// must not answer it, or it would yank the camera and controls off the lander
// mid-descent. Hence `__player_flies_elsewhere` below: possession is checked, never
// assumed, and control is never stolen from a vessel the player already holds.
//
// Driving is a control loop in on_tick (a signal task), and it's the prelude's
// `nav_to` — which already carries the skid-drive sign convention, the hard turn
// when the gate is behind, and the throttle ease-off that makes the rover pivot
// instead of arcing wide. See `scripting/prelude/nav.rhai`.

/// Waypoint position by index, discovered via parent(me). `parent(me)` is the
/// scene root (the rover is a direct child of the assembly root), and the waypoints
/// are siblings: `/<SceneRoot>/RoverTarget1`, etc.
fn wp_pos(i) {
    let root = parent(me);
    if root == () { return (); }
    let path = "/" + name(root) + "/RoverTarget" + i;
    world_pos(find(path))
}

/// True if a HUMAN session drives `id`. `controller` reports the driver's role —
/// "AiAgent" for an autopilot, "Owner"/"Operator" for a person — so this is the
/// human-vs-AI test, not a bare `is_controlled`. `find` yields -1 when the prim is
/// absent; guard for it.
fn human_drives(id) {
    if id < 0 { return false; }
    let role = controller(id);
    role != () && role != "AiAgent"
}

/// True if the player is at the controls of some OTHER vessel right now. Scans
/// siblings (children of the scene root) for any entity that a human currently
/// drives. This is scene-agnostic: it finds the lander in the lander scene, or
/// any other vessel the player might be flying in a different scene.
fn __player_flies_elsewhere(me) {
    let root = parent(me);
    if root == () { return false; }
    for sib in children(root) {
        if sib != me && human_drives(sib) { return true; }
    }
    false
}

/// The side-effects of standing down. `this.manual` is NOT set here: the runtime
/// binds `this` only in the hook it calls directly, never in the functions that
/// hook calls (see `scripting/prelude/nav.rhai`), so each caller latches it itself.
fn __stand_down(me) {
    brake(me);
    notify_kind("Manual control - autopilot off.", "info");
}

fn on_tick(me) {
    if this.active != true { return; }   // not deployed yet
    if this.manual == true { return; }   // player took over
    // The player possessed the rover themselves (clicked it / pressed F). Yield —
    // and possess nothing: they already own it.
    if human_drives(me) {
        this.manual = true;
        __stand_down(me);
        return;
    }
    if this.i == () { this.i = 1; }
    if this.i > 3 { brake(me); return; } // course complete

    let target = wp_pos(this.i);
    if target == () { return; }          // waypoint not loaded yet

    // Steer there; nav_to returns true (and brakes) once inside the gate radius.
    if nav_to(me, target, 0.7, 4.0) { this.i += 1; }
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.active = true;
        this.i = 1;
        return;
    }
    if !evt.name.starts_with("key:") { return; }
    if this.active != true || this.manual == true { return; }
    // Keys meant for the lander stay with the lander.
    if __player_flies_elsewhere(me) { return; }
    // Nothing else is possessed: the key IS the takeover request. Hand the rover
    // over — possess it for the player so their keys reach it.
    possess(me);
    this.manual = true;
    __stand_down(me);
}
