// Lander onboard SUPERVISOR — bound by `LunCoProgramAPI` on the `Subsystems` Scope
// on the lander asset (`vessels/landers/descent_lander.usda`), whose
// `uniform asset info:sourceAsset = @scenarios/lander_subsystems.rhai@` names
// this file, so it rides along into any scene that references one. The runtime
// binds the program to its owning vessel, so `me` is that vessel; nothing here
// names a scene path. EVENT-DRIVEN only: no control loop, no per-tick routing.
//
// Control is entirely WIRING-LEVEL (no rhai stepping):
//   • The airframe's flight-control system (`Lander.mo`) turns a COMMAND into force
//     and torque. It holds no descent law: an airframe flies what it is told.
//   • The command comes either from a session's stick or from a guidance program the
//     SCENE composed onto the vessel (`components/gnc/descent_guidance.usda`), wired
//     into the airframe's `guidance_throttle` input. Unwired, it is zero.
//   • Authority is possession (the WIRED `piloted` port, derived by `PILOTED_BACKEND`
//     in Rust), not a flag — nothing here touches it.
//
// This script only ORCHESTRATES via events: it latches source-qualified engine
// cutoff at target-qualified gear load, latches flight handoff at low-speed
// four-pad contact, observes the later quiet touchdown, and reacts to native
// Modelica fuel events. The typed GNC inputs retain each flight mode after its
// event; this script never runs a physics loop.

fn own_gnc(me) {
    // `me` is the vessel owner selected by the LunCoProgramAPI binding, even
    // though the source prim is the vessel's `Subsystems` child.
    let lander_name = name(me);
    if lander_name == () { return -1; }
    find(lander_name + "/GNC")
}

fn on_event(me, evt) {
    if evt.name == "lander_engine_cutoff"
        || evt.name == "lander_flight_handoff"
        || evt.name == "lander_touchdown" {
        let gnc = own_gnc(me);
        if gnc < 0 {
            print("[Lander] landing transition failed: guidance program is missing");
            return;
        }
        if evt.name == "lander_engine_cutoff" {
            // TelemetryEvent is a shared bus. Only the GNC instance that emitted
            // this typed transition may change its own mode; the other lander
            // must keep flying its independently tuned trajectory.
            if evt.source != gnc { return; }
            // The USD event is authored as latched, and the Modelica input is
            // the authoritative per-GNC state. Do not keep a second mutable
            // latch in this shared supervisor script.
            if !set(gnc, "landing_engine_cutoff_latched", 1.0) {
                print("[Lander] landing transition failed: engine-cutoff latch is not writable");
                return;
            }
            print("[Lander] Target-qualified gear load - main engine cutoff latched.");
            return;
        }
        if evt.name == "lander_flight_handoff" {
            if evt.source != gnc { return; }
            if !set(gnc, "landing_handoff_latched", 1.0) {
                print("[Lander] landing transition failed: handoff latch is not writable");
                return;
            }
            if !set(gnc, "engage", 0.0) {
                print("[Lander] landing transition failed: GNC engage port is not writable");
                return;
            }
            print("[Lander] Four-pad contact - flight control handed to landing gear.");
            return;
        }
        // Touchdown is a physical fact emitted by the airframe, not permission
        // to mutate either GNC latch. Its source is the owning lander, while
        // the two guidance transitions above are sourced by the GNC child.
        if evt.source != me { return; }
        print("[Lander] Touchdown - landing gear compressed and quiet.");
        return;
    }
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
        print("[Lander] LOW FUEL.");
    } else if evt.name == "lander_depleted" {
        notify_kind("Propellant depleted.", "warn");
        print("[Lander] Propellant depleted.");
    }
}
