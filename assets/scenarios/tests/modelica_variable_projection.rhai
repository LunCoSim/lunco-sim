fn simulation_dependencies(me, ctx) {
    let engine = find_path("/RocketEngineObservables/Engine");
    #{ modelica_entities: [engine], entity_reads: [engine], entity_writes: [engine],
       query_reads: ["ReadModelicaStepSamples"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.engine = find_path("/RocketEngineObservables/Engine");
    this.done = false;
    this.elapsed = 0.0;
    this.phase = 0;
    let variables = get(this.engine, "ModelicaVariables.values");
    this.initial = variables.thrust;
    this.paused = set(this.engine, "ModelicaModel.paused", true);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    this.phase += 1;
    if this.phase == 1 { return; }
    if this.phase == 2 {
        let variables = get(this.engine, "ModelicaVariables.values");
        this.retained = variables.thrust == this.initial && get(this.engine, "thrust") == this.initial;
        this.command_ok = cmd("SetPorts", #{target: this.engine, writes: [["throttle", 1.0]]}).ok;
        this.resumed = set(this.engine, "ModelicaModel.paused", false);
        return;
    }
    let record = query("ReadModelicaStepSamples", #{targets: [#{api_id: this.engine,
        input_names: ["throttle"], output_names: ["thrust"]}]});
    let sample = record.entities[0].sample;
    if sample == () && this.elapsed < 1.0 { return; }
    let variables = get(this.engine, "ModelicaVariables.values");
    this.done = true;
    report_verdict([
        t_true(this.paused && this.resumed, "the isolated participant paused and resumed"),
        t_true(this.retained, "the paused native snapshot remained published"),
        t_true(this.command_ok, "the public input command was accepted"),
        t_true(sample != (), "the worker admitted a subsequent communication result"),
        t_true(sample != () && abs(sample.output_time_s - sample.input_time_s - dt()) < 1.0e-9,
               "the omitted communication period exchanges on each fixed physics tick"),
        t_true(sample != () && sample.sampled_inputs.throttle == 1.0,
               "the admitted result consumed the commanded input"),
        t_true(sample != () && variables.thrust == sample.sampled_outputs.thrust
               && get(this.engine, "thrust") == variables.thrust && variables.thrust > this.initial,
               "the native reflected snapshot and public port match the accepted solver result"),
    ], "MODELICA VARIABLE PROJECTION", "MODELICA_VARIABLE_PROJECTION");
}
