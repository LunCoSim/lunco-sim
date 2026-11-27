// A finite subnormal drive torque must not stop an unloaded raycast axle.
// The API fixture admits the rover above the contact range with zero gravity.
fn simulation_dependencies(me, ctx) {
    let rover = find_path(ctx.rover_path);
    let reads = [rover]; let writes = [rover];
    for corner in ["FL", "FR", "RL", "RR"] {
        let wheel = find_path(ctx.rover_path + "/Suspension_" + corner + "/Wheel_" + corner);
        reads.push(wheel); writes.push(wheel);
    }
    #{modelica_entities: [], entity_reads: reads, entity_writes: writes,
      query_reads: ["GetPort", "QueryPhysicsState", "GetReadiness", "RuntimeDiagnostics"], required_inputs: []}
}

fn sample(wheel, name) {
    let result = query("GetPort", #{api_id: wheel, name: name});
    if result.value == () { throw("missing wheel port: " + name); }
    result.value
}

// Rhai uses an epsilon comparison when the operand product underflows.
// Rescale first so even the smallest nonzero f64 differs from zero by more
// than that epsilon, and adjacent original subnormals remain distinguishable.
fn scaled(value) { value * 1.0e308 }

fn on_start(me, ctx) {
    this = #{done: false, phase: 0, mark: 0, checks: [], wheels: [], torques: []};
    for corner in ["FL", "FR", "RL", "RR"] {
        this.wheels.push(find_path(ctx.rover_path + "/Suspension_" + corner + "/Wheel_" + corner));
        this.torques.push(if corner == "FL" || corner == "RL" { 4.837e-321 } else { -4.837e-321 });
    }
    this.rover = find_path(ctx.rover_path);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let readiness = query("GetReadiness");
    if !readiness.ready || readiness.world_hold { return; }
    if this.phase == 0 {
        let state = query("QueryPhysicsState", #{id: this.rover});
        if state.wheel_contacts.len() != 4 { return; }
        let unloaded = true;
        for contact in state.wheel_contacts {
            unloaded = unloaded && !contact.contact_valid && contact.normal_force_n == 0.0;
        }
        this.checks.push(t_true(unloaded, "all four wheel axles are unloaded"));
        let accepted = true;
        for i in 0..this.wheels.len() {
            accepted = accepted && cmd("SetPorts", #{target: this.wheels[i], writes: [["drive", this.torques[i]]]}).ok;
        }
        this.checks.push(t_true(accepted, "subnormal torque commands accepted"));
        this.mark = sim_tick(); this.phase = 1; return;
    }
    if sim_tick() - this.mark < 10 { return; }
    if this.phase == 1 {
        let retained = true; let moving = true;
        for i in 0..this.wheels.len() {
            let wheel = this.wheels[i]; let torque = this.torques[i]; let speed = sample(wheel, "shaft_speed");
            let drive_scaled = scaled(sample(wheel, "drive"));
            let torque_scaled = scaled(torque);
            let speed_scaled = scaled(speed);
            retained = retained && torque_scaled != 0.0 && drive_scaled == torque_scaled;
            moving = moving && abs(speed_scaled) < 1.0 && (if torque_scaled > 0.0 { speed_scaled > 0.0 } else { speed_scaled < 0.0 });
            emit("SUBNORMAL_AXLE_SAMPLE", #{wheel: wheel, drive_nm: sample(wheel, "drive"), speed_radps: speed});
        }
        this.checks.push(t_true(retained, "wheel ports retain the exact nonzero torque"));
        this.checks.push(t_true(moving, "both signs of subnormal shaft motion are preserved"));
        this.checks.push(t_true(sim_tick() > this.mark, "native simulation ticks continue"));
        this.checks.push(t_true(brake(this.rover).ok, "brake command accepted"));
        this.mark = sim_tick(); this.phase = 2; return;
    }
    let stopped = true;
    for wheel in this.wheels { stopped = stopped && scaled(sample(wheel, "shaft_speed")) == 0.0; }
    this.checks.push(t_true(stopped, "brake brings subnormal speeds to rest without reversal"));
    this.checks.push(t_true(!readiness.faulted && query("RuntimeDiagnostics").errors == 0,
        "simulation remains healthy after the brake transition"));
    this.done = true;
    report_verdict(this.checks, "SUBNORMAL AXLE BALANCE", "SUBNORMAL_AXLE_BALANCE");
}
