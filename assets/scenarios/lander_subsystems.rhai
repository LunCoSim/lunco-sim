// Lander onboard SUPERVISOR — bound by `LunCoProgramAPI` on the `Subsystems` Scope
// on the lander asset (`vessels/landers/descent_lander.usda`), whose
// `uniform asset info:sourceAsset = @scenarios/lander_subsystems.rhai@` names
// this file, so it rides along into any scene that references one. Everything below resolves relative to `me`; nothing here names a
// scene path. EVENT-DRIVEN only: no control loop, no per-tick routing.
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
// This script only ORCHESTRATES via events: it latches touchdown for the mission to
// react to, and reacts to the native Modelica fuel `when` events.

fn on_event(me, evt) {
    if evt.name == "lander_touchdown" {
        print("[Lander] Touchdown - settled on legs.");
        return;
    }
    // AUTHORITY is now a first-class WIRED signal: the vessel's `piloted` port
    // (1 when ANY session — a user or an autopilot — possesses it, derived from the
    // possession registry in Rust) is wired straight to the internal GNC model,
    // which yields on it. No script toggle here — the internal controller reads
    // possession status directly through the wire.

    // Fuel events — threshold crossings on the model's `m_prop` port, declared as
    // Connected `LunCoEvent` prims on the lander (`LowFuel`, `Depleted`).
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
        print("[Lander] LOW FUEL.");
    } else if evt.name == "lander_depleted" {
        notify_kind("Propellant depleted.", "warn");
        print("[Lander] Propellant depleted.");
    }
}
