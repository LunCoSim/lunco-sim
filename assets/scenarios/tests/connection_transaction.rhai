// @timing presentation
// A complete input exchange must reject the whole batch before native physics
// consumes a bad mass or a centre-of-mass coordinate from the same rejected communication point.
// com_x sorts before mass: the valid write must remain staged when mass fails.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetPort", "GetReadiness", "GetBrokenConnections", "PhysicsPerformance"], required_inputs: []}
}
fn require(result) { if result.ok != true { throw(result.to_string()); } }
fn settled() {
    let clock = clock_snapshot();
    !clock.barrier_held && !clock.simulation_progress.held && !clock.simulation_cycle_active
        && clock.queued_simulation_steps.to_int() == 0
}
fn value(target, name) {
    let result = query("GetPort", #{api_id: target, name: name});
    if result.value == () { throw(result.to_string()); }
    result.value
}
fn on_start(me, ctx) {
    this.checks = []; this.done = false;
    require(cmd("StepSimulation", #{hold: true}));
}
fn task(me, ctx) {
    seq([
        wait_until(|m| find_path("/World/Source") >= 0 && find_path("/World/Target") >= 0 && settled()),
        once(|m| {
            this.source = find_path("/World/Source"); this.target = find_path("/World/Target");
            require(cmd("StepSimulation", #{steps: 3}));
        }),
        wait_until(|m| settled()),
        once(|m| {
            this.mass = value(this.target, "mass"); this.com = value(this.target, "com_x");
            this.native_steps = query("PhysicsPerformance").step_number;
            this.source_time = value(this.source, "sampleTime");
            this.checks += [
                t_true(this.mass > 0.0 && this.mass < 5.0, "positive Modelica samples reach native mass before rejection"),
                t_true(this.com > 0.02, "the companion centre of mass advances during valid exchanges"),
                t_true(value(this.source, "mass") < 0.0, "the next communication point contains the real invalid source sample")
            ];
            require(cmd("StepSimulation", #{steps: 1}));
        }),
        wait_until(|m| query("GetReadiness").faulted || settled()),
        once(|m| {
            let ready = query("GetReadiness"); let wiring = query("GetBrokenConnections");
            let rejected = ();
            for broken in wiring.broken {
                if broken.port == "mass" { rejected = broken.dropped_value; }
            }
            this.checks += [
                t_true(ready.faulted, "a rejected live exchange raises a terminal runtime fault"),
                t_true(ready.faulted && ready.fault.kind == "cosim-input-write-rejected", "the co-simulation input owner identifies the rejection"),
                t_true(value(this.target, "mass") == this.mass, "the rejected batch preserves the last accepted native mass"),
                t_true(value(this.target, "com_x") == this.com, "the rejected batch preserves its otherwise valid companion input"),
                t_true(query("PhysicsPerformance").step_number == this.native_steps, "native physics does not integrate the failed communication point"),
                t_true(value(this.source, "sampleTime") == this.source_time, "Modelica does not advance beyond the rejected shared communication point"),
                t_true(wiring.runtime_fault != () && wiring.runtime_fault.kind == "cosim-input-write-rejected", "the public wiring report retains the terminal failure"),
                t_true(type_of(rejected) == "map" && rejected.kind == "finite"
                    && rejected.value == value(this.source, "mass"),
                    "the diagnostic retains the exact finite rejected sample")
            ];
            this.done = true;
            report_verdict(this.checks, "CONNECTION TRANSACTION", "CONNECTION_TRANSACTION");
        })
    ])
}
