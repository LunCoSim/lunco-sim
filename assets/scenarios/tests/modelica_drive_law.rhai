// Positive acceptance test for the authored Modelica drivetrain.
//
// The scenario owns motion acceptance. Rust only projects the generic USD
// topology and port graph; the composed Modelica controller publishes the
// final drive values consumed by the wheel realization.

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/ModelicaDriveLaw/RoverModelica");
    #{ modelica_entities: [rover], entity_reads: [rover], entity_writes: [rover], query_reads: [], required_inputs: [] }
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
    this.rover = find_path("/ModelicaDriveLaw/RoverModelica");
    this.start = ();
    if this.rover < 0 {
        this.done = true;
        report_verdict(["Modelica rover did not compose"],
                       "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
        return;
    }
    let claim = cmd("AcquireControl", #{target: this.rover, bind_camera: false});
    this.claimed = claim != () && claim.ok == true;
    if !this.claimed {
        this.done = true;
        report_verdict([t_true(false, "Modelica drive gate could not acquire manual authority")],
                       "MODELICA DRIVE LAW", "MODELICA_DRIVE_LAW");
        return;
    }
    this.start = world_pos(this.rover);
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 1.0 {
        brake(this.rover);
        return;
    }

    drive(this.rover, 1.0, 0.0);
    if this.elapsed < 8.0 { return; }

    let left = port_value(this.rover, "drive_left", "out");
    let right = port_value(this.rover, "drive_right", "out");
    let travel = vlen(vsub(world_pos(this.rover), this.start));
    brake(this.rover);
    let checks = [
        t_true(this.claimed, "Modelica drive gate acquired manual authority"),
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
    if this.claimed { brake(this.rover); release_control(this.rover); }
}
