// Production acceptance for the bundled RocketEngine model.
//
// The observable contract is intentionally checked through USD and Rhai:
// source resolution, Modelica worker lifecycle, and output collection all
// remain in the same path used by a real authored scene.

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [find_path("/RocketEngineObservables/Engine")], entity_reads: [], entity_writes: [],
       query_reads: ["ReadModelicaStepSamples"], required_inputs: [] }
}

fn accepted_step(engine) {
    let record = query("ReadModelicaStepSamples", #{targets: [#{api_id: engine,
        input_names: ["throttle"], output_names: ["thrust"]}]});
    if record == () || record.entities == () || record.entities.len() != 1 { return (); }
    record.entities[0].sample
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.engine = find_path("/RocketEngineObservables/Engine");
    this.idle_mass = ();
    this.burn_mass = ();
    this.impulse = ();
    this.m_dot = ();
    this.thrust = ();
    this.p_chamber = ();
    this.isp = ();
    this.command_ok = false;
    this.command_step = ();
    this.first_command_step = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    if this.engine < 0 {
        this.done = true;
        report_verdict(["RocketEngine program did not compose"],
                       "ROCKET ENGINE OBSERVABLES", "ROCKET_ENGINE_OBSERVABLES");
        return;
    }

    if this.phase == 0 {
        if this.elapsed < 1.0 { return; }
        this.idle_mass = get(this.engine, "m_prop");
        this.command_step = accepted_step(this.engine);
        let command = cmd("SetPorts", #{
            target: this.engine,
            writes: [["throttle", 1.0]],
        });
        this.command_ok = command.ok;
        this.phase = 1;
        this.elapsed = 0.0;
        return;
    }

    // Read the first accepted solver advance after the Rhai command. Eventual
    // thrust alone cannot detect a schedule that consumes the command late.
    if this.first_command_step == () && this.command_step != () {
        let sample = accepted_step(this.engine);
        if sample != () && sample.step_id > this.command_step.step_id {
            this.first_command_step = sample;
        }
    }
    if this.elapsed < 2.0 { return; }
    this.burn_mass = get(this.engine, "m_prop");
    this.impulse = get(this.engine, "impulse");
    this.m_dot = get(this.engine, "m_dot");
    this.thrust = get(this.engine, "thrust");
    this.p_chamber = get(this.engine, "p_chamber");
    this.isp = get(this.engine, "isp");

    let failures = [];
    failures.push(t_true(this.command_ok, "throttle command was rejected"));
    failures.push(t_true(this.command_step != () && this.first_command_step != ()
        && this.first_command_step.input_time_s == this.command_step.output_time_s
        && this.first_command_step.sampled_inputs.throttle == 1.0,
        "first solver advance after SetPorts did not consume throttle=1: "
        + this.first_command_step));
    failures.push(t_present(this.idle_mass, "initial propellant output"));
    failures.push(t_present(this.burn_mass, "burn propellant output"));
    failures.push(t_present(this.impulse, "total impulse output"));
    failures.push(t_present(this.m_dot, "mass-flow output"));
    failures.push(t_present(this.thrust, "thrust output"));
    failures.push(t_present(this.p_chamber, "chamber-pressure output"));
    failures.push(t_present(this.isp, "specific-impulse output"));
    if this.idle_mass != () && this.burn_mass != () {
        failures.push(t_true(this.burn_mass < this.idle_mass,
            "propellant did not decrease at full throttle"));
    }
    if this.m_dot != () {
        failures.push(t_true(this.m_dot > 10.0,
            "mass flow was not positive at full throttle"));
    }
    if this.thrust != () {
        failures.push(t_true(this.thrust > 30000.0,
            "thrust was not physically positive at full throttle"));
    }
    if this.p_chamber != () {
        failures.push(t_true(this.p_chamber > 3000000.0,
            "chamber pressure was not positive at full throttle"));
    }
    if this.isp != () {
        failures.push(t_true(this.isp > 300.0,
            "specific impulse was not exposed"));
    }
    if this.impulse != () {
        failures.push(t_true(this.impulse > 10000.0,
            "integrated impulse did not increase"));
    }
    print("[rocket-engine] m_dot=" + r2(this.m_dot) +
          " thrust=" + r2(this.thrust) +
          " p_chamber=" + r2(this.p_chamber) +
          " isp=" + r2(this.isp));
    this.done = true;
    report_verdict(failures, "ROCKET ENGINE OBSERVABLES", "ROCKET_ENGINE_OBSERVABLES");
}
