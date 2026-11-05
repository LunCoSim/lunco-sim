// @timing presentation
// Run on an admitted Modelica scene; retained across the negative clear case.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ReadExposures", "GetReadiness", "ListTelemetryChannels", "QueryTelemetryHistory"], required_inputs: []}
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
        this.checks.push(t_true(value.modelica_worker_ms_per_physics_step != ()
            && value.modelica_worker_ms_per_physics_step.is_finite() && value.modelica_worker_ms_per_physics_step > 0.0,
            "running Modelica has no finite positive worker time per physics step"));
        this.checks.push(t_true(value.modelica_worker_step_ms != () && value.modelica_worker_step_ms > 0.0,
            "running Modelica worker has no measured service time"));
        this.checks.push(t_true(value.modelica_sample_wall_secs != () && value.modelica_sample_wall_secs >= 1.0
            && value.modelica_sample_physics_steps != () && value.modelica_sample_physics_steps != "0",
            "Modelica timing has no complete wall-clock/physics-step window"));
        this.checks.push(t_true(value.modelica_participants == this.participants.to_string(),
            "Modelica performance participant count disagrees with the scene"));
        let channels = query("ListTelemetryChannels", #{name: "engine.modelica_worker_ms_per_physics_step"}).channels;
        this.checks.push(t_true(channels.len == 1, "Modelica engine timing telemetry channel is missing"));
        if channels.len == 1 {
            let history = query("QueryTelemetryHistory", #{key: channels[0].key, limit: 10});
            let valid = history.samples.len > 0;
            for sample in history.samples { valid = valid && sample.v.is_finite() && sample.v > 0.0; }
            this.checks.push(t_true(valid, "Modelica engine timing history has no positive samples"));
        } else {
            this.checks.push(t_true(false, "Modelica engine timing history is unavailable"));
        }
        for name in ["engine.fps", "engine.frame_time", "engine.physics_fps", "engine.physics_step_time", "engine.modelica_fps", "engine.modelica_worker_step_time"] {
            let matches = query("ListTelemetryChannels", #{name: name}).channels;
            let valid = matches.len == 1;
            if valid {
                let history = query("QueryTelemetryHistory", #{key: matches[0].key, limit: 10});
                valid = history.samples.len > 0;
                for sample in history.samples { valid = valid && sample.v.is_finite() && sample.v > 0.0; }
            }
            this.checks.push(t_true(valid, "missing positive engine telemetry samples for " + name));
        }
        cmd("SetTimeTransport", #{playing: false, rate: 1.0});
        this.phase = 1;
        this.since = clock_snapshot().wall_elapsed_s;
    } else if this.phase == 1 {
        this.checks.push(t_true(value.modelica_worker_ms_per_physics_step == (),
            "paused physics retains a Modelica cost per step without completed physics steps"));
        this.checks.push(t_true(value.modelica_worker_step_ms == (),
            "an idle worker publishes a mean for an empty sample window"));
        cmd("ClearScene", #{});
        this.phase = 2;
        this.since = clock_snapshot().wall_elapsed_s;
    } else {
        let ready = query("GetReadiness", #{}).ready;
        if !ready && elapsed < 15.0 { return; }
        this.checks.push(t_true(ready, "cleared scene did not settle"));
        this.checks.push(t_true(value.modelica_worker_ms_per_physics_step == (),
            "negative empty-scene case retains Modelica time per physics step"));
        this.checks.push(t_true(value.modelica_worker_step_ms == (),
            "negative empty-scene case retains Modelica worker timing"));
        this.checks.push(t_true(value.modelica_participants == "0",
            "negative empty-scene case retains a Modelica participant"));
        this.done = true;
        report_verdict(this.checks, "MODELICA PERFORMANCE HUD", "MODELICA_PERFORMANCE_HUD");
    }
}
