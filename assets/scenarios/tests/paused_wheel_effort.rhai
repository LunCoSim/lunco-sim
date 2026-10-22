// Suspension freezes the complete drivetrain after it has advanced normally.
// A supervisor outside that graph observes the stopped models and live world.

fn simulation_dependencies(me, ctx) {
    let reads = vessel_descendants(find_path("/World/Physical"));
    for entity in vessel_descendants(find_path("/World/Raycast")) { reads.push(entity); }
    reads.push(find_path("/World/Independent"));
    reads.push(find_path("/World/RemoteDrive"));
    reads.push(find_path("/World/IndependentModel"));
    reads.push(find_path("/World/Independent/Controller"));
    #{ modelica_entities: [find_path("/World/Physical"), find_path("/World/Raycast"),
                          find_path("/World/RemoteDrive"), find_path("/World/IndependentModel")],
       entity_reads: reads, entity_writes: [find_path("/World/Physical"), find_path("/World/Raycast")],
       query_reads: ["GetReadiness", "RuntimeDiagnostics"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.sent = false;
    this.model_times = [];
    this.controllers = [find_path("/World/Physical"),
        find_path("/World/Raycast"), find_path("/World/Independent")];
    this.controller_ticks = [0, 0, 0];
    this.settled_ticks = [];
    this.start_y = world_pos(find_path("/World/Independent"))[1];
}

fn on_event(me, evt, ctx) {
    if evt.name != "test:object-controller-tick" { return; }
    for i in 0..this.controllers.len() {
        if evt.source == this.controllers[i] { this.controller_ticks[i] += 1; }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if !this.sent && this.elapsed >= 0.1 {
        for name in ["Physical", "Raycast"] {
            let root = find_path("/World/" + name);
            this.model_times.push(get(root, "ModelicaModel.current_time"));
            let result = cmd("PauseSimulationObject", #{ target: root,
                condition: "test:absorber-exhausted",
                detail: "Authored test policy suspends the complete live drivetrain" });
            if result.ok != true { throw("wheel assembly pause was rejected: " + result); }
        }
        let remote = find_path("/World/RemoteDrive");
        this.remote_time = get(remote, "ModelicaModel.current_time");
        this.remote_state = get(remote, "accumulated");
        this.sent = true;
    }
    if this.elapsed >= 0.2 && this.settled_ticks.len() == 0 {
        this.settled_ticks = this.controller_ticks;
    }
    if this.elapsed < 0.4 { return; }
    let checks = [];
    let index = 0;
    for name in ["Physical", "Raycast"] {
        let root = find_path("/World/" + name);
        let state = query("QueryPhysicsState", #{ id: root });
        checks.push(t_true(state.physics_object_paused && state.rigid_body_disabled
            && !state.physics_state_pending,
            name + " policy pause disables the object without blocking readiness"));
        checks.push(t_true(this.model_times[index] > 0.0
            && get(root, "ModelicaModel.current_time") == this.model_times[index]
            && get(root, "ModelicaModel.paused") == true
            && state.simulation_object_paused,
            name + " complete Modelica drivetrain retains its last accepted state and clock"));
        index += 1;
    }
    for suffix in ["FL", "FR", "RL", "RR"] {
        let path = "/World/Physical/Suspension_" + suffix + "/Wheel_" + suffix;
        let state = query("QueryPhysicsState", #{ id: find_path(path) });
        checks.push(t_true(state.physics_object_paused && state.rigid_body_disabled,
            "paused shaft cannot apply effort to wheel " + suffix));
    }
    let readiness = query("GetReadiness", #{});
    checks.push(t_true(readiness.ready && !readiness.world_hold && !readiness.faulted,
        "paused wheel effort does not fault the application"));
    checks.push(t_true(world_pos(find_path("/World/Independent"))[1] < this.start_y - 0.01,
        "independent admitted physics continues integrating"));
    let diagnostics = query("RuntimeDiagnostics", #{});
    let coupling_error = false;
    for finding in diagnostics.findings {
        if finding.code == "cosim-joint-coupling-invalid" { coupling_error = true; }
    }
    checks.push(t_true(!coupling_error, "object suspension is not malformed effort coupling"));
    let remote = find_path("/World/RemoteDrive");
    checks.push(t_true(this.remote_time > 0.0
        && get(remote, "ModelicaModel.current_time") == this.remote_time
        && get(remote, "accumulated") == this.remote_state
        && get(remote, "ModelicaModel.paused") == true,
        "port-connected model outside the body hierarchy stops with the drivetrain"));
    let independent = find_path("/World/IndependentModel");
    checks.push(t_true(get(independent, "ModelicaModel.current_time") > 0.3
        && get(independent, "accumulated") > 0.3
        && get(independent, "ModelicaModel.paused") == false,
        "independent continuous model retains its running clock and state"));
    for i in 0..2 {
        checks.push(t_true(this.settled_ticks[i] > 0
            && this.controller_ticks[i] == this.settled_ticks[i],
            "suspended object controller stops executing: " + i));
    }
    checks.push(t_true(this.controller_ticks[2] > this.settled_ticks[2],
        "independent object controller continues executing"));
    this.done = true;
    report_verdict(checks, "PAUSED WHEEL EFFORT", "PAUSED_WHEEL_EFFORT");
}
