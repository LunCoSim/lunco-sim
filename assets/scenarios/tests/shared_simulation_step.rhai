// @timing presentation

fn probe() {
    import "lunco://scenarios/tests/modelica_initialization_network.rhai" as network;
    network::row(query("CosimStatus").entities, "ClockProbe")
}
fn settled() {
    let clock = clock_snapshot();
    let model = probe();
    model != () && (model.status == "Idle" || model.status == "Running")
        && clock.queued_simulation_steps.to_int() == 0
        && !clock.barrier_held && !clock.simulation_cycle_active
}
fn on_start(me, ctx) {
    import "lunco://scenarios/tests/modelica_initialization_network.rhai" as network;
    subscribe("SHARED_STEP_PRESENTATION");
    this.failures = [];
    this.done = false;
    this.doc = ();
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.title == "shared_simulation_step"
            && doc.runtime_context.kind == "local_twin" { this.doc = doc.doc_id; }
    }
    if this.doc == () { throw("shared stepping fixture document is missing"); }
    let held = cmd("StepSimulation", #{hold: true});
    if held.ok != true { throw(held.to_string()); }
    let attached = network::attach(this.doc, "ClockProbe", 0.5, 0.0);
    if attached.ok != true { throw(attached.to_string()); }
}
fn baseline(me) {
    this.start = clock_snapshot();
    this.model_start = probe().modelica_current_time;
}
fn verify_frozen(me) {
    let clock = clock_snapshot();
    let wrong_clock_rejected = false;
    try { dt(); } catch (error) { wrong_clock_rejected = true; }
    this.failures += [
        t_true(wrong_clock_rejected, "simulation time helpers reject the presentation cadence"),
        t_true(execution_context().clock == "presentation", "task retains its authored presentation clock"),
        t_true(clock.clock_contract_ok, "the shared clock contract is valid while held"),
        t_true(clock.sim_tick == this.start.sim_tick, "a hold freezes simulation ticks"),
        t_true(clock.physics_elapsed_s == this.start.physics_elapsed_s, "a hold freezes native integration"),
        t_true(probe().modelica_current_time == this.model_start, "a hold freezes Modelica time")
    ];
}
fn verify_steps(me, count) {
    let clock = clock_snapshot();
    let elapsed = count * this.start.fixed_dt_s;
    this.failures += [
        t_true(clock.clock_contract_ok, "stepped clocks retain the shared contract"),
        t_true(clock.sim_tick - this.start.sim_tick == count, "the requested count admits exact complete cycles"),
        t_true(abs(clock.fixed_elapsed_s - this.start.fixed_elapsed_s - elapsed) < 1.0e-12,
            "manual cycles retain the fixed physics quantum"),
        t_true(abs(clock.physics_elapsed_s - this.start.physics_elapsed_s - elapsed) < 1.0e-12,
            "native integration consumes every shared cycle"),
        t_true(abs(probe().modelica_current_time - this.model_start - elapsed) < 1.0e-8,
            "Modelica consumes the same elapsed interval"),
        t_true(abs(probe().outputs.solverTime - probe().modelica_current_time) < 1.0e-8,
            "Modelica time uses the committed solver coordinate")
    ];
}
fn three_steps(me) {
    let result = cmd("StepSimulation", #{steps: 3});
    if result.ok != true { throw(result.to_string()); }
}
fn paused_steps(me) {
    this.verify_steps(me, 3);
    cmd("SetTimeTransport", #{playing: false, rate: 4.0});
    cmd("StepSimulation", #{hold: false});
    this.baseline(me);
    let result = cmd("StepSimulation", #{steps: 2});
    if result.ok != true { throw(result.to_string()); }
}
fn paused_check(me) {
    this.verify_steps(me, 2);
    let clock = clock_snapshot();
    this.failures += [
        t_true(!clock.transport_playing && clock.transport_rate == 4.0,
            "manual admission preserves user pause and rate"),
        t_true(!clock.manual_simulation_hold, "paused stepping needs no manual hold")
    ];
    this.baseline(me);
}
fn invalid_requests(me) {
    this.verify_frozen(me);
    let invalid = cmd("StepSimulation", #{hold: true, steps: 0});
    this.failures.push(t_true(invalid.ok == false && !clock_snapshot().manual_simulation_hold
        && clock_snapshot().queued_simulation_steps.to_int() == 0,
        "zero cycles reject atomically without acquiring a hold"));
    cmd("SetTimeTransport", #{playing: true, rate: 1.0});
    let unheld = cmd("StepSimulation", #{steps: 1});
    this.failures.push(t_true(unheld.ok == false && clock_snapshot().queued_simulation_steps.to_int() == 0,
        "automatic running rejects ambiguous extra cycles"));
    this.done = true;
    report_verdict(this.failures, "SHARED SIMULATION STEP", "SHARED_SIMULATION_STEP");
}
fn task(me, ctx) {
    seq([
        wait_until(|m| settled()),
        once(|m| emit("SHARED_STEP_PRESENTATION", true)), wait_for("SHARED_STEP_PRESENTATION"),
        once(Fn("baseline")), wait(0.05),
        once(Fn("verify_frozen")), once(Fn("three_steps")),
        wait_until(|m| settled()), once(Fn("paused_steps")),
        wait_until(|m| settled()), once(Fn("paused_check")), wait(0.05),
        once(Fn("invalid_requests"))
    ])
}
