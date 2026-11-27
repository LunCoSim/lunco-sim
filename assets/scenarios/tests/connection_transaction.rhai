// @timing presentation
// A complete input exchange must reject the whole batch before native physics
// consumes a bad mass or a centre-of-mass coordinate from the same rejected communication point.
// com_x sorts before mass: the valid write must remain staged when mass fails.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetPort", "GetReadiness", "GetBrokenConnections", "PhysicsPerformance", "QueryTelemetryHistory"], required_inputs: []}
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
            this.start = clock_snapshot();
            let channel = "exchange_mass_probe";
            this.mass_key = "api/" + this.target + ":" + channel;
            this.position_key = "api/" + this.target + ":position.x";
            require(cmd("ControlTelemetry", #{entity: this.target, channel: channel, port: "mass",
                unit: "kg", rate_hz: 1.0 / this.start.simulation_dt_s}));
            require(cmd("StepSimulation", #{steps: 3}));
        }),
        wait_until(|m| settled()),
        wait(0.1),
        once(|m| {
            this.mass = value(this.target, "mass"); this.com = value(this.target, "com_x");
            this.completed = clock_snapshot();
            this.mass_samples = query("QueryTelemetryHistory", #{key: this.mass_key}).samples;
            this.position_samples = query("QueryTelemetryHistory", #{key: this.position_key}).samples;
            this.native_steps = query("PhysicsPerformance").step_number;
            this.source_time = value(this.source, "sampleTime");
            this.checks += [
                t_true(this.completed.clock_contract_ok && this.completed.sim_tick - this.start.sim_tick == 3
                    && this.completed.physics_elapsed_s == this.completed.fixed_elapsed_s
                    && abs(this.completed.fixed_elapsed_s - this.start.fixed_elapsed_s - 3.0 * this.completed.fixed_dt_s) < 1.0e-12,
                    "three accepted ticks advance the fixed and native clocks together"),
                t_true(abs(this.source_time - 3.0 * this.completed.simulation_dt_s) < 1.0e-12
                    && abs(this.source_time - this.completed.world_sim_s) < 1.0e-12,
                    "the Modelica sample and scene time reach the same logical endpoint: " + this.source_time + ", " + this.completed.world_sim_s),
                t_true(this.mass > 0.0 && this.mass < 5.0, "positive Modelica samples reach native mass before rejection"),
                t_true(this.com > 0.02, "the companion centre of mass advances during valid exchanges"),
                t_true(value(this.source, "mass") < 0.0, "the next communication point contains the real invalid source sample")
            ];
            require(cmd("StepSimulation", #{steps: 1}));
        }),
        wait_until(|m| query("GetReadiness").faulted || settled()),
        wait(0.1),
        once(|m| {
            let clock = clock_snapshot();
            let ready = query("GetReadiness"); let wiring = query("GetBrokenConnections");
            let rejected = ();
            for broken in wiring.broken {
                if broken.port == "mass" { rejected = broken.dropped_value; }
            }
            this.checks += [
                t_true(clock.clock_contract_ok && !clock.simulation_cycle_active, "the stopped world exposes a consistent completed clock"),
                t_true(this.mass_samples.len() == 3 && this.mass_samples[2].v == this.mass
                    && abs(this.mass_samples[2].t - this.completed.world_sim_s) < 1.0e-12
                    && query("QueryTelemetryHistory", #{key: this.mass_key}).samples == this.mass_samples,
                    "mass telemetry retains the three integrated samples exactly after the failed tick"),
                t_true(!this.position_samples.is_empty()
                    && query("QueryTelemetryHistory", #{key: this.position_key}).samples == this.position_samples,
                    "native position history keeps the complete accepted series across rejection"),
                t_true(clock.sim_tick == this.completed.sim_tick && clock.fixed_elapsed_s == this.completed.fixed_elapsed_s
                    && clock.physics_elapsed_s == this.completed.physics_elapsed_s,
                    "a rejected exchange preserves master, fixed and native time (" + this.completed.sim_tick + " -> " + clock.sim_tick + ")"),
                t_true(clock.world_sim_s == this.completed.world_sim_s && clock.world_met_s == this.completed.world_met_s,
                    "mission and world time retain the last completed interval"),
                t_true(ready.faulted, "a rejected live exchange raises a terminal runtime fault"),
                t_true(ready.faulted && ready.fault.kind == "cosim-input-write-rejected", "the co-simulation input owner identifies the rejection"),
                t_true(value(this.target, "mass") == this.mass, "the rejected batch preserves the last accepted native mass"),
                t_true(value(this.target, "com_x") == this.com, "the rejected batch preserves its otherwise valid companion input"),
                t_true(query("PhysicsPerformance").step_number == this.native_steps, "native physics does not integrate the failed communication point"),
                t_true(value(this.source, "sampleTime") == this.source_time, "Modelica does not advance beyond the rejected shared communication point"),
                t_true(wiring.runtime_fault != () && wiring.runtime_fault.kind == "cosim-input-write-rejected"
                    && wiring.runtime_fault.global_id == this.target && wiring.runtime_fault.entity_name != (),
                    "the public wiring report retains the terminal failure and registered target identity"),
                t_true(type_of(rejected) == "map" && rejected.kind == "finite"
                    && rejected.value == value(this.source, "mass"),
                    "the diagnostic retains the exact finite rejected sample")
            ];
            this.done = true;
            report_verdict(this.checks, "CONNECTION TRANSACTION", "CONNECTION_TRANSACTION");
        })
    ])
}
