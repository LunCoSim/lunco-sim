// Positive acceptance test for the authored Modelica drivetrain.
//
// The scenario owns motion acceptance. Rust only projects the generic USD
// topology and port graph; the composed Modelica controller publishes the
// final drive values consumed by the wheel realization.

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/ModelicaDriveLaw/RoverModelica");
    let avatar = find_path("/ModelicaDriveLaw/Avatar");
    #{ modelica_entities: [rover], entity_reads: [rover, avatar], entity_writes: [rover, avatar], query_reads: [], required_inputs: [] }
}

fn port_value(target, name, direction) {
    let record = query("ReadPorts", #{ api_id: target, port_names: [name] });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == direction { return port.value; }
    }
    ()
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    this.claimed = false;
    this.manual_idle = true;
    this.manual_live = false;
    this.selection_idle = false;
    this.neutral_requested = false;
    this.manual_neutral = false;
    this.program_started = false;
    this.rover = find_path("/ModelicaDriveLaw/RoverModelica");
    this.avatar = find_path("/ModelicaDriveLaw/Avatar");
    this.start = ();
    if this.rover < 0 {
        this.done = true;
        report_verdict(["Modelica rover did not compose"],
                       "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
        return;
    }
    cmd("SetPorts", #{ target: this.rover,
        writes: [["throttle", 1.0], ["steer", 0.0], ["brake", 0.0]] });
    this.start = world_pos(this.rover);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 1.0 {
        let left = port_value(this.rover, "drive_left", "out");
        this.manual_idle = this.manual_idle && left != () && left.abs() < 0.000001;
        return;
    }
    if !this.claimed && !this.program_started {
        let claim = cmd("AcquireControl", #{target: this.rover, source: this.avatar, bind_camera: true});
        if claim == () || claim.ok != true {
            fail_fast("Modelica drive gate could not acquire manual authority",
                "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
            this.done = true;
            return;
        }
        this.claimed = true;
        this.selection_idle = port_value(this.rover, "piloted", "out") == 0.0;
        cmd("SimulateIntent", #{target: this.rover, intent: "forward", held: true});
    }
    if this.elapsed < 2.0 {
        let left = port_value(this.rover, "drive_left", "out");
        if left != () && left > 0.5
                && port_value(this.rover, "piloted", "out") == 1.0 {
            this.manual_live = true;
        }
        return;
    }
    if !this.neutral_requested {
        cmd("SimulateIntent", #{target: this.rover, intent: "forward", held: false});
        this.neutral_requested = true;
        return;
    }
    if this.elapsed < 2.25 {
        this.manual_neutral = port_value(this.rover, "piloted", "out") == 1.0
            && port_value(this.rover, "throttle", "in") == 0.0;
        return;
    }
    if !this.program_started {
        release_control(this.rover);
        cmd("ReleaseControlSource", #{source: this.avatar});
        this.claimed = false;
        this.program_started = true;
    }

    drive(this.rover, 1.0, 0.0);
    if this.elapsed < 8.0 { return; }

    let left = port_value(this.rover, "drive_left", "out");
    let right = port_value(this.rover, "drive_right", "out");
    let travel = vlen(vsub(world_pos(this.rover), this.start));
    brake(this.rover);
    let checks = [
        t_true(this.manual_idle, "unowned manual input cannot actuate the drivetrain"),
        t_true(this.selection_idle, "possession without controller input preserves guidance authority"),
        t_true(this.manual_live, "claimed manual input actuates the drivetrain"),
        t_true(this.manual_neutral, "neutral key release retains manual authority"),
        t_true(port_value(this.rover, "piloted", "out") == 0.0
            && left != () && left > 0.5,
            "direct program input actuates without session possession"),
        t_present(left, "Modelica drive_left output"),
        t_present(right, "Modelica drive_right output"),
        t_true(travel > 0.5, "Modelica controller did not move the rover"),
    ];
    print("[modelica-drive-law] travel=" + r2(travel) + "m");
    release_control(this.rover);
    this.claimed = false;
    this.done = true;
    report_verdict(checks, "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
}

fn on_stop(me, ctx) {
    if this.claimed {
        brake(this.rover);
        release_control(this.rover);
        cmd("ReleaseControlSource", #{source: this.avatar});
    }
}
