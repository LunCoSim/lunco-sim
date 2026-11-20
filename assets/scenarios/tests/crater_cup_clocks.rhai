// @timing presentation
// Observe the playable base from outside the fixed loop, after worker receipts settle.
fn mission_participants() { [
    "/World/Scout", "/World/Comet", "/World/Miner", "/World/Tailings", "/World/Tanker",
    "/World/Works/PackingLine", "/World/RelayPlant", "/World/HopperLander/JetPhotometry"
] }

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CosimStatus", "GetReadiness"], required_inputs: []}
}

fn issue_step(params) {
    let result = cmd("StepSimulation", params);
    if !result.ok { throw(result.to_string()); }
}

fn settled() {
    let clock = clock_snapshot();
    !clock.simulation_cycle_active && !clock.barrier_held && !clock.simulation_progress.held
        && clock.queued_simulation_steps.to_int() == 0
}

fn on_start(me, ctx) {
    this.checks = [];
    this.was_held = clock_snapshot().manual_simulation_hold;
}

fn check_endpoint(steps, phase) {
    let clock = clock_snapshot();
    let scene_time = (clock.sim_tick - clock.mission_tick0).to_float() * clock.simulation_dt_s;
    let expected_native = this.start.physics_elapsed_s + steps.to_float() * this.start.fixed_dt_s;
    this.checks.push(t_true(clock.clock_contract_ok && clock.manual_simulation_hold && settled()
        && !query("GetReadiness").faulted && clock.sim_tick == this.start.sim_tick + steps
        && clock.fixed_elapsed_s == clock.physics_elapsed_s
        && abs(clock.physics_elapsed_s - expected_native) < 1.0e-9
        && abs(clock.world_sim_s - scene_time) < 1.0e-12,
        phase + " expected " + steps + " completed ticks from " + this.start.sim_tick
            + ", observed " + clock.sim_tick + ", native seconds " + clock.physics_elapsed_s));

    let participants = mission_participants();
    let report = query("CosimStatus", #{include_values: false});
    let names = [];
    let models = #{};
    for row in report.entities {
        if row.modelica_communication_period_secs != () || row.modelica_var_count > 0 || row.modelica_error_present {
            names.push(row.name);
            models[row.name] = row;
        }
    }
    this.checks.push(t_true(report.synchronization.topology_ready && names.len() == participants.len()
        && clock.barrier_active_participants == participants.len(),
        phase + " expected eight participants, found " + names));
    for path in participants {
        let row = models[path];
        this.checks.push(t_true(row != () && row.status == "Running" && !row.modelica_paused
            && !row.modelica_is_stepping && !row.modelica_error_present
            && row.modelica_communication_period_secs != ()
            && abs(row.modelica_communication_period_secs - clock.simulation_dt_s) < 1.0e-12
            && abs(row.modelica_current_time - scene_time) < 1.0e-12
            && abs(row.modelica_target_time - scene_time) < 1.0e-12
            && abs(row.modelica_next_communication_time - (scene_time + clock.simulation_dt_s)) < 1.0e-12,
            phase + " " + path + " expected solver and target at " + scene_time
                + " s, followed by one communication period"));
    }
}

fn task(me, ctx) {
    seq([
        wait_until(|m| query("GetReadiness").ready && clock_snapshot().barrier_active_participants == mission_participants().len()),
        once(|m| issue_step(#{hold: true})),
        wait_until(|m| settled()),
        once(|m| { this.start = clock_snapshot(); issue_step(#{steps: 7}); }),
        wait_until(|m| settled()), wait(0.05),
        once(|m| this.check_endpoint(7, "Seven manual steps")),
        wait(0.20),
        once(|m| { this.check_endpoint(7, "Holding"); issue_step(#{steps: 1}); }),
        wait_until(|m| settled()), wait(0.05),
        once(|m| {
            this.check_endpoint(8, "One further step");
            issue_step(#{hold: this.was_held});
            report_verdict(this.checks, "CRATER CUP CLOCKS", "CRATER_CUP_CLOCKS");
        })
    ])
}
