// Production runtime acceptance for the bundled multi-class Modelica example.
//
// The source is a package containing a nested runnable class. The USD scene
// authors the legal flat public boundary; this script owns the stimulus and
// observable runtime verdict. Parser and annotation implementation details are
// owned by their focused unit tests, not by this scene test.

fn on_start(me) {
    this.stage = find("/AnnotatedRocketStageTest/RocketStage");
    this.elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.idle_mass = ();
    this.burn_mass = ();
    this.burn_thrust = ();
    this.burn_flow = ();
    this.coast_mass = ();
    this.command_ok = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.stage < 0 {
        this.done = true;
        report_verdict(["AnnotatedRocketStage program did not compose"],
                       "ANNOTATED ROCKET STAGE", "ANNOTATED_ROCKET_STAGE");
        return;
    }

    if this.phase == 0 {
        if this.elapsed < 1.0 { return; }
        this.idle_mass = get(this.stage, "tank_mass");
        let command = cmd("SetPorts", #{
            target: this.stage,
            writes: [["valve_opening", 100.0]],
        });
        this.command_ok = command.ok;
        this.phase = 1;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 1 {
        if this.elapsed < 8.0 { return; }
        this.burn_mass = get(this.stage, "tank_mass");
        this.burn_thrust = get(this.stage, "thrust");
        this.burn_flow = get(this.stage, "mass_flow");
        let command = cmd("SetPorts", #{
            target: this.stage,
            writes: [["valve_opening", 0.0]],
        });
        this.command_ok = this.command_ok && command.ok;
        this.phase = 2;
        this.elapsed = 0.0;
        return;
    }

    if this.elapsed < 3.0 { return; }
    this.coast_mass = get(this.stage, "tank_mass");
    let altitude = get(this.stage, "altitude");
    let velocity = get(this.stage, "velocity");
    let availability = get(this.stage, "tank_availability");
    let failures = [];
    failures.push(t_true(this.command_ok,
        "public valve_opening command was rejected"));
    failures.push(t_present(this.idle_mass, "initial tank_mass output"));
    failures.push(t_present(this.burn_mass, "burn tank_mass output"));
    failures.push(t_present(this.burn_thrust, "thrust output"));
    failures.push(t_present(this.burn_flow, "mass_flow output"));
    failures.push(t_present(this.coast_mass, "coast tank_mass output"));
    failures.push(t_present(altitude, "altitude output"));
    failures.push(t_present(velocity, "velocity output"));
    failures.push(t_present(availability, "tank_availability output"));
    if this.idle_mass != () && this.burn_mass != () {
        failures.push(t_true(this.burn_mass < this.idle_mass,
            "tank mass did not decrease during the commanded burn"));
    }
    if this.burn_thrust != () {
        failures.push(t_true(this.burn_thrust > 0.0,
            "full opening produced no thrust"));
    }
    if this.burn_flow != () {
        failures.push(t_true(this.burn_flow > 0.0,
            "full opening produced no positive outgoing mass flow"));
    }
    if this.burn_mass != () && this.coast_mass != () {
        failures.push(t_true((this.coast_mass - this.burn_mass).abs() < 1.0,
            "closed valve did not stop tank drain"));
    }
    failures.push(t_true(altitude != () && altitude > 0.0,
        "rocket altitude did not rise after ignition"));
    failures.push(t_true(velocity != () && velocity > 0.0,
        "rocket velocity did not rise after ignition"));
    print("[annotated-rocket-stage] mass=" + r2(this.burn_mass) +
          " thrust=" + r2(this.burn_thrust) +
          " flow=" + r2(this.burn_flow) +
          " altitude=" + r2(altitude));
    this.done = true;
    report_verdict(failures, "ANNOTATED ROCKET STAGE", "ANNOTATED_ROCKET_STAGE");
}
