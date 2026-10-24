// Authored initial conditions must retain the scene axes through site admission.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [find_path("/World/Flight"),
        find_path("/World/Companion"), find_path("/World/GlobeDrop"),
        find_path("/World/CollarDrop")], entity_writes: [],
        query_reads: ["QueryPhysicsState"], required_inputs: [] }
}

fn on_start(me, ctx) { this.started = sim_tick(); this.done = false; }

fn on_tick(me, ctx) {
    if this.done || sim_tick() - this.started < 3 { return; }
    this.done = true;
    let flight = query("QueryPhysicsState", #{id: find_path("/World/Flight")});
    let companion = query("QueryPhysicsState", #{id: find_path("/World/Companion")});
    let drop = query("QueryPhysicsState", #{id: find_path("/World/GlobeDrop")});
    let lateral = query("QueryPhysicsState", #{id: find_path("/World/CollarDrop")});
    emit("AUTHORED_INITIAL_VELOCITY_EVIDENCE", #{flight: flight.linear_velocity_mps,
        companion: companion.linear_velocity_mps, drop: drop.linear_velocity_mps,
        lateral: lateral.linear_velocity_mps, angular: drop.angular_velocity_radps});
    report_verdict([
        t_true(abs(flight.linear_velocity_mps[0]) < 0.01 && abs(flight.linear_velocity_mps[2]) < 0.01
            && flight.linear_velocity_mps[1] > 79.7 && flight.linear_velocity_mps[1] <= 80.0,
            "authored upward velocity remains in the admitted lunar site axes"),
        t_true(abs(companion.linear_velocity_mps[0]) < 0.01 && abs(companion.linear_velocity_mps[2]) < 0.01
            && companion.linear_velocity_mps[1] > 79.7 && companion.linear_velocity_mps[1] <= 80.0,
            "joint preparation preserves the attached body's authored initial velocity"),
        t_true(abs(drop.linear_velocity_mps[0]) < 0.01 && abs(drop.linear_velocity_mps[2]) < 0.01
            && drop.linear_velocity_mps[1] < -150.0 && drop.linear_velocity_mps[1] > -150.3,
            "negative authored velocity retains its site direction"),
        t_true(abs(lateral.linear_velocity_mps[0] - 30.0) < 0.01
            && abs(lateral.linear_velocity_mps[2]) < 0.01 && abs(lateral.linear_velocity_mps[1]) < 0.3,
            "lateral authored velocity does not acquire celestial-frame components"),
        t_true(abs(drop.angular_velocity_radps[0]) < 0.001 && abs(drop.angular_velocity_radps[2]) < 0.001
            && abs(drop.angular_velocity_radps[1] - PI()/3.0) < 0.001,
            "authored angular velocity retains its axes and degree-per-second conversion")
    ], "AUTHORED INITIAL VELOCITY", "AUTHORED_INITIAL_VELOCITY");
}
