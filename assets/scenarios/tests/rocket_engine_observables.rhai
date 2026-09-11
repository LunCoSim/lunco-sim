// Production acceptance for the bundled RocketEngine model.
//
// The observable contract is intentionally checked through USD and Rhai:
// source resolution, Modelica worker lifecycle, and output collection all
// remain in the same path used by a real authored scene.

fn on_start(me) {
    this.elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.engine = find("/RocketEngineObservables/Engine");
    this.idle_mass = ();
    this.burn_mass = ();
    this.impulse = ();
    this.m_dot = ();
    this.thrust = ();
    this.p_chamber = ();
    this.isp = ();
    this.command_ok = false;
}

fn on_tick(me) {
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
        let command = cmd("SetPorts", #{
            target: this.engine,
            writes: [["throttle", 1.0]],
        });
        this.command_ok = command.ok;
        this.phase = 1;
        this.elapsed = 0.0;
        return;
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
