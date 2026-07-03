// Lander onboard SUPERVISOR — referenced from USD by `lunco:scriptPath` on
// `/LanderTest/Lander`. EVENT-DRIVEN only: no control loop, no per-tick routing.
//
// Control is entirely WIRING-LEVEL now (no rhai stepping):
//   • The GNC law (Modelica) outputs `gnc_throttle`; a USD self-wire
//     (`gnc_throttle:external_throttle`) drives the actuator every tick in the cosim
//     propagation — realtime, no script involved.
//   • A possessing human's control binding writes the same command inputs and
//     overrides the GNC wire (human > gnc). Authority is possession, not a flag.
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
    // AUTHORITY (gnc < human), EVENT-DRIVEN: on any possession change, re-evaluate
    // who drives THIS lander and flip the model's `manual` gate. `controller(me)`
    // reads the possession arbiter (ground truth), so a human possessing it → manual
    // 1 (their stick drives), releasing / possessing something else → manual 0 (GNC
    // resumes). No per-tick loop — this fires only on the possession events.
    if evt.name == "cmd:PossessVessel" || evt.name == "cmd:ReleaseVessel" {
        let drv = controller(me);
        let m = if drv != () && drv != "AiAgent" { 1.0 } else { 0.0 };
        set(me, "manual", m);
        print("[Lander] authority: manual=" + m + " (driver=" + drv + ")");
    }

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
