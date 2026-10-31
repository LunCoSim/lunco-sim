// @timing presentation
// Run on an admitted Modelica scene; retained across the negative clear case.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ReadExposures", "GetReadiness"], required_inputs: []}
}

fn health() {
    query("ReadExposures", #{surface: "engine-health"}).surfaces["engine-health"].properties
}

fn on_start(me, params) {
    this.participants = params.participants;
    this.checks = [];
    this.phase = 0;
    this.since = clock_snapshot().wall_elapsed_s;
    this.done = false;
    cmd("SetTimeTransport", #{playing: true, rate: 1.0});
}

fn on_tick(me, ctx) {
    if this.done { return; }
    let elapsed = clock_snapshot().wall_elapsed_s - this.since;
    if elapsed < 3.0 { return; }
    let value = health();
    if this.phase == 0 {
        this.checks.push(t_true(value.modelica_fps != ()
            && value.modelica_fps.is_finite() && value.modelica_fps > 0.0,
            "running Modelica has no finite positive completion FPS"));
        this.checks.push(t_true(value.modelica_worker_step_ms != () && value.modelica_worker_step_ms > 0.0,
            "running Modelica worker has no measured service time"));
        this.checks.push(t_true(value.modelica_sample_wall_secs != () && value.modelica_sample_wall_secs >= 1.0,
            "Modelica FPS has no complete native wall-clock window"));
        this.checks.push(t_true(value.modelica_participants == this.participants.to_string(),
            "Modelica performance participant count disagrees with the scene"));
        cmd("SetTimeTransport", #{playing: false, rate: 1.0});
        this.phase = 1;
        this.since = clock_snapshot().wall_elapsed_s;
    } else if this.phase == 1 {
        this.checks.push(t_true(value.modelica_fps == 0.0,
            "paused Modelica retains a running completion rate"));
        this.checks.push(t_true(value.modelica_worker_step_ms == (),
            "an idle worker publishes a mean for an empty sample window"));
        cmd("ClearScene", #{});
        this.phase = 2;
        this.since = clock_snapshot().wall_elapsed_s;
    } else {
        let ready = query("GetReadiness", #{}).ready;
        if !ready && elapsed < 15.0 { return; }
        this.checks.push(t_true(ready, "cleared scene did not settle"));
        this.checks.push(t_true(value.modelica_fps == (),
            "negative empty-scene case retains Modelica FPS"));
        this.checks.push(t_true(value.modelica_worker_step_ms == (),
            "negative empty-scene case retains Modelica worker timing"));
        this.checks.push(t_true(value.modelica_participants == "0",
            "negative empty-scene case retains a Modelica participant"));
        this.done = true;
        report_verdict(this.checks, "MODELICA PERFORMANCE HUD", "MODELICA_PERFORMANCE_HUD");
    }
}
