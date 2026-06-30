// Lander-local subsystem monitor — referenced from USD by `lunco:scriptPath` on
// the `/LanderTest/Lander` prim. It runs ON the lander entity ALONGSIDE that
// entity's Modelica GNC model: one entity carrying a MODEL and a SCRIPT at once
// (doc 34, Decision 1 — model = continuous control, script = supervisory logic).
//
// It reads OUTSIDE DATA — the GNC model's own published ports (`m_prop`
// propellant, exposed as a co-sim output on this same entity) — instead of
// re-deriving state in rhai. Event-driven and latched; no time integration.

fn on_tick(me) {
    // Propellant remaining (kg): a Modelica observable surfaced as a port on
    // this entity. `get` falls back to the port read for a bare name.
    let prop = get(me, "m_prop");
    if prop == () { return; }

    // Low-fuel warning — latched so it fires once, not every tick.
    if prop < 200.0 && this.low_fuel != true {
        this.low_fuel = true;
        emit("LANDER_LOW_FUEL", prop);
        print("[Lander] LOW FUEL: " + prop + " kg remaining.");
    }

    // Auto-disarm on empty so a dry engine can't command phantom thrust. Cut via
    // the port path (clobber-safe through SimComponent.inputs).
    if prop <= 0.0 && this.dry != true {
        this.dry = true;
        set(me, "engine_enable", 0.0);
        emit("LANDER_PROPELLANT_DEPLETED", 0);
        print("[Lander] Propellant depleted — engine disarmed.");
    }
}
