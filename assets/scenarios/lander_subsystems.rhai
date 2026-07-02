// Lander onboard software — referenced from USD by `lunco:scriptPath` on the
// `/LanderTest/Lander` prim; runs ON the lander entity. Two jobs: (1) AUTO-LAND
// GUIDANCE (below) — the GNC now lives here as rhai policy and WRITES the command
// ports (`external_throttle`/`pitch`/`roll`/`yaw`), since `Lander.mo` is now a
// pure commands->force actuator (auto and manual share one command interface);
// (2) supervisory monitoring — touchdown latch + fuel-event reactions.
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

    // ── AUTO-LAND GUIDANCE ──────────────────────────────────────────────────
    // The lander's GNC now lives HERE (rhai policy), not in Lander.mo — the model
    // is a pure commands->force actuator. We read pose + velocity and WRITE the
    // same command ports a pilot would (`external_throttle`/`pitch`/`roll`/`yaw`),
    // so auto and manual share one interface. NOTE: manual-takeover arbitration is
    // a higher layer (TODO) — today the autopilot flies until touchdown, then idles.
    if this.touchdown != true && pos != () {
        // Horizontal target: the landing pad, else hold current x/z (pure descent).
        let tx = pos[0];
        let tz = pos[2];
        if target_ent != () {
            let tp = world_pos(target_ent);
            if tp != () { tx = tp[0]; tz = tp[2]; }
        }
        // World velocities from the cosim state ports (() -> 0 if unavailable).
        let vy = get(me, "velocity_y"); if vy == () { vy = 0.0; }
        let vx = get(me, "velocity_x"); if vx == () { vx = 0.0; }
        let vz = get(me, "velocity_z"); if vz == () { vz = 0.0; }
        let alt = pos[1];

        // Vertical: descent-rate schedule (fast up high, gentle near ground) +
        // throttle = hover feed-forward (m*g/F_max ~= 0.054) + PD on descent rate.
        // Kept as flat `let`s so no single expression trips rhai's expr-depth cap.
        let vy_sched = clamp(alt * 0.25, 0.4, 6.0);
        let target_vy = -vy_sched;
        let hover = (2000.0 * 1.62) / 60000.0;
        let thr = hover + 0.05 * (target_vy - vy);
        set(me, "external_throttle", clamp(thr, 0.0, 1.0));

        // Horizontal: tilt to null position error + damp lateral drift, assuming
        // near-upright small angles (world +X ~ roll, +Z ~ pitch). Gentle gains,
        // kept small so it never flips the lander; live-tune later.
        let ex = tx - pos[0];
        let ez = tz - pos[2];
        let roll_cmd = 0.05 * ex - 0.20 * vx;
        let pitch_cmd = 0.05 * ez - 0.20 * vz;
        set(me, "roll", clamp(roll_cmd, -0.4, 0.4));
        set(me, "pitch", clamp(pitch_cmd, -0.4, 0.4));
        set(me, "yaw", 0.0);
    } else if this.touchdown == true {
        // Settled on the legs — cut thrust so it rests.
        set(me, "external_throttle", 0.0);
    }

    // Touchdown: settled on the legs (low altitude + near-zero vertical rate).
    // Latched so it emits exactly once. Detection is hoisted to a top-level fn so
    // on_tick's nesting stays under rhai's expr-depth cap (prelude-style split).
    if this.touchdown != true && __lander_settled(me, target_ent) {
        this.touchdown = true;
        emit("lander_touchdown");
        print("[Lander] Touchdown - settled on legs.");
    }
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
