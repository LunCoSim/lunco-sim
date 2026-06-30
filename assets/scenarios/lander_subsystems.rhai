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
    let target_ent = find("/LanderTest/LandingLocation");

    // If the lander has touchdown but is moved back up into the air (Y > 5.5),
    // reset touchdown latch and re-arm the engine so the pilot can fly it again.
    let pos = world_pos(me);
    if pos != () && pos[1] > 5.5 {
        if this.touchdown == true {
            this.touchdown = false;
            set(me, "engine_enable", 1.0);
            print("[Lander] Lander moved in the air - re-arming engine and resetting touchdown.");
        }
    }

    // Touchdown: settled on the legs (low altitude + near-zero vertical rate).
    // Latched so it emits exactly once. The data owner detects; others react.
    if this.touchdown != true {
        let should_touchdown = false;
        if target_ent == () {
            should_touchdown = true;
        } else {
            let target_pos = world_pos(target_ent);
            if target_pos != () && target_pos[1] <= 0.1 {
                should_touchdown = true;
            }
        }

        if should_touchdown {
            let alt_ent = find("/LanderTest/Lander/Altimeter");
            let alt = if alt_ent != () { get(alt_ent, "range") } else { () };
            let pos = world_pos(me);
            let vy = get(me, "velocity_y");

            let is_low = false;
            if alt != () {
                // Nozzle-mounted altimeter is at Y=-3.3, pads at Y=-5.0.
                // When pads touch the ground, altimeter range is 1.7m.
                is_low = alt < 2.0;
            } else if pos != () {
                // Fallback to ideal coordinates
                is_low = pos[1] < 5.3;
            }

            if is_low && vy != () && vy > -0.1 && vy < 0.1 {
                this.touchdown = true;
                emit("lander_touchdown");
                print("[Lander] Touchdown - settled on legs.");
            }
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
