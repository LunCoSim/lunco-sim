// Lander-local subsystem monitor — referenced from USD by `lunco:scriptPath` on
// the `/LanderTest/Lander` prim. Runs ON the lander entity ALONGSIDE its Modelica
// GNC model (doc 34, Decision 1 — model = continuous control, script = supervisory).
//
// EVENT-DRIVEN, no port polling:
//  - Fuel thresholds are NATIVE Modelica `when` events now (`lunco:portEvents` on
//    the Lander prim turns `m_prop` crossings into `lander_low_fuel` /
//    `lander_depleted`); this script only REACTS to them in on_event.
//  - Touchdown is a COMPOUND condition (low AND slow) — not a single port edge —
//    so it stays a latched rhai edge here, and is emitted as `lander_touchdown`
//    for the mission scenario to wait on.

fn on_tick(me) {
    // Touchdown: settled on the legs (low altitude + near-zero vertical rate).
    // Latched so it emits exactly once. The data owner detects; others react.
    if this.touchdown != true {
        let pos = world_pos(me);
        // Read the avian `velocity_y` PORT, not velocity()/LinearVelocity — a
        // cosim'd body doesn't expose LinearVelocity to reflection, so velocity()
        // returns () and the old guard never passed. The port is wired + reliable.
        let vy = get(me, "velocity_y");
        if pos != () && vy != () && pos[1] < 3.4 && vy > -0.1 && vy < 0.1 {
            this.touchdown = true;
            emit("lander_touchdown");
            print("[Lander] Touchdown - settled on legs.");
        }
    }
}

fn on_event(me, evt) {
    // React to the native Modelica port-edge events (from `lunco:portEvents`).
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
        print("[Lander] LOW FUEL.");
    } else if evt.name == "lander_depleted" {
        // Auto-disarm a dry engine so it can't command phantom thrust. Cut via
        // the port path (clobber-safe through SimComponent.inputs).
        set(me, "engine_enable", 0.0);
        notify_kind("Propellant depleted - engine disarmed.", "warn");
        print("[Lander] Propellant depleted - engine disarmed.");
    }
}
