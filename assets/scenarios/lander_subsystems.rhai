// Lander onboard supervisory software — referenced from USD by `lunco:scriptPath`
// on `/LanderTest/Lander`, runs ON the lander entity. The GNC control LAW now
// lives in `Lander.mo` (Modelica); this script is HIGH-LEVEL POLICY ONLY — no
// control math:
//   (1) AUTHORITY GATE — hand the lander from the GNC to the pilot on manual input
//       by writing the model's `manual` port (gnc < human). Re-arm to the GNC when
//       lifted back into the air. This is the flag-policy that flips the model's
//       one authority bit on events; the arbitration order is DATA in the USD
//       `lunco:controlLayers`, the selection math is the model's arithmetic gate.
//   (2) SUPERVISORY — latch touchdown (a compound low-AND-slow predicate) and react
//       to the native Modelica fuel `when` events (`lunco:portEvents`).
//
// EVENT-DRIVEN: no per-tick control loop here anymore — the GNC flies itself.

fn on_start(me) {
    this.touchdown = false;
    this.manual = false;
    set(me, "manual", 0.0);        // GNC drives by default — the lowest authority tier
    set(me, "engine_enable", 1.0); // armed for the autonomous descent
}

fn on_tick(me) {
    let pos = world_pos(me);

    // Lifted back into the air (Y > 5.5) → reset to a fresh autonomous descent:
    // re-arm the engine and hand control back to the GNC (manual = 0).
    if pos != () && pos[1] > 5.5 && this.touchdown == true {
        this.touchdown = false;
        this.manual = false;
        set(me, "engine_enable", 1.0);
        set(me, "manual", 0.0);
        print("[Lander] Lifted - re-arming engine, GNC re-engaged.");
    }

    // Touchdown latch: settled on the legs (low + slow). A PREDICATE, not control
    // math. Emits exactly once for the mission scenario to wait on; the mission
    // then safes the engine. Hoisted to a top-level fn so on_tick stays shallow.
    let target_ent = find("/LanderTest/LandingLocation");
    if this.touchdown != true && __lander_settled(me, target_ent) {
        this.touchdown = true;
        emit("lander_touchdown");
        print("[Lander] Touchdown - settled on legs.");
    }
}

fn on_event(me, evt) {
    // AUTHORITY: a flight-control keypress hands the lander from the GNC to the
    // pilot (human outranks gnc) — the same yield-on-input rule the rover autopilot
    // uses. Flip the model's `manual` gate to 1 so it actuates the external command.
    // (Full possession-aware arbitration arrives when scenarios can observe vessel
    // ownership; until then this best-effort key rule covers single-player.)
    if this.touchdown != true && this.manual != true && __is_flight_key(evt.name) {
        this.manual = true;
        set(me, "manual", 1.0);
        notify_kind("Manual override - you have the lander.", "info");
        print("[Lander] Manual override - pilot has control.");
    }

    // Native Modelica fuel `when` events (from `lunco:portEvents`).
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
        print("[Lander] LOW FUEL.");
    } else if evt.name == "lander_depleted" {
        // Auto-disarm a dry engine (clobber-safe via the port path).
        set(me, "engine_enable", 0.0);
        notify_kind("Propellant depleted - engine disarmed.", "warn");
        print("[Lander] Propellant depleted - engine disarmed.");
    }
}

// The flight-control keys that mean "the pilot is flying the lander now".
fn __is_flight_key(name) {
    name == "key:KeyW" || name == "key:KeyS" || name == "key:KeyA" ||
    name == "key:KeyD" || name == "key:Space" || name == "key:ShiftLeft"
}

// True once the lander has settled: within range of the pad (or no pad), low
// altitude, and near-zero vertical rate. Hoisted out of `on_tick` (fresh function
// body = shallow nesting) with early returns so no single expression is too deep.
fn __lander_settled(me, target_ent) {
    let arrived = target_ent == ();
    if !arrived {
        let tp = world_pos(target_ent);
        arrived = tp != () && tp[1] <= 0.1;
    }
    if !arrived { return false; }

    // Prefer the nozzle altimeter's range; fall back to ideal Y. (Altimeter at
    // Y=-3.3, pads at Y=-5.0 → range ~1.7m at contact.)
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
    vy > -0.1 && vy < 0.1
}
