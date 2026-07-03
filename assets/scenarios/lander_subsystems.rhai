// Lander onboard SUPERVISOR — referenced from USD by `lunco:scriptPath` on
// `/LanderTest/Lander`. EVENT-DRIVEN only: no control loop, no per-tick routing.
//
// Control is entirely WIRING-LEVEL now (no rhai stepping):
//   • The internal GNC law lives in `Lander.mo` and actuates the vessel directly.
//   • It YIELDS to its pilot via the WIRED `piloted` port (1 when a session — a user
//     or an autopilot — possesses the vessel). A possessing session's control
//     binding writes the command inputs; the model's `piloted` gate selects them.
//   • Authority is possession (derived by `PILOTED_BACKEND` in Rust), not a flag —
//     nothing here touches it.
//
// This script only ORCHESTRATES via events: it latches touchdown for the mission to
// react to, and reacts to the native Modelica fuel `when` events.

fn on_tick(me) {
    // Touchdown latch: a compound low-AND-slow predicate the mission waits on.
    // (Supervisory detection, not control — a contact-sensor `when` event is the
    // planned upgrade so even this leaves on_tick.) Emits exactly once.
    if this.touchdown != true && __lander_settled(me) {
        this.touchdown = true;
        emit("lander_touchdown");
        print("[Lander] Touchdown - settled on legs.");
    }
}

fn on_event(me, evt) {
    // AUTHORITY is now a first-class WIRED signal: the vessel's `piloted` port
    // (1 when ANY session — a user or an autopilot — possesses it, derived from the
    // possession registry in Rust) is wired straight to the internal GNC model,
    // which yields on it. No script toggle here — the internal controller reads
    // possession status directly through the wire.

    // Native Modelica fuel `when` events (from `lunco:portEvents`).
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
        print("[Lander] LOW FUEL.");
    } else if evt.name == "lander_depleted" {
        notify_kind("Propellant depleted.", "warn");
        print("[Lander] Propellant depleted.");
    }
}

// True once the lander has settled: low altitude (altimeter range) + near-zero
// vertical rate. Hoisted out of `on_tick` (fresh body = shallow nesting).
fn __lander_settled(me) {
    let alt_ent = find("/LanderTest/Lander/Altimeter");
    let alt = ();
    if alt_ent != () { alt = get(alt_ent, "range"); }
    let is_low = false;
    if alt != () {
        is_low = alt < 2.0;
    } else {
        let pos = world_pos(me);
        is_low = pos != () && pos[1] < 5.3;
    }
    if !is_low { return false; }

    let vy = get(me, "velocity_y");
    if vy == () { return false; }
    vy > -0.2 && vy < 0.2
}
