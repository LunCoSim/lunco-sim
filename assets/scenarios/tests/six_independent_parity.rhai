// Authored acceptance test for the six-wheel independent-drive rover.
//
// The six output connections and their bank routing are USD/Modelica policy.
// This Rhai test checks the resulting command surface and observable motion;
// it does not inspect or recreate a Rust steering algorithm.

fn port_value(target, name, direction) {
    let record = query("ReadPorts", #{ api_id: target });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == direction { return port.value; }
    }
    ()
}

fn authored_asset_contract(rover) {
    vessel_wheels_match(rover, 6, 0, 0.4, 0.7, 15000.0, 3000.0, 1.5)
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.rover = find("/SixIndependentParity/Rover");
    this.start = ();
    this.asset_contract = false;
    if this.rover < 0 {
        this.done = true;
        report_verdict(["six-wheel rover did not compose"],
                       "SIX-WHEEL CONTROLLER", "SIX_WHEEL_CONTROLLER");
        return;
    }
    this.start = world_pos(this.rover);
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 1.0 {
        brake(this.rover);
        return;
    }

    drive(this.rover, 1.0, 0.0);
    if !this.asset_contract { this.asset_contract = authored_asset_contract(this.rover); }
    if this.elapsed < 8.0 { return; }

    let w0 = port_value(this.rover, "drive_w0", "out");
    let w1 = port_value(this.rover, "drive_w1", "out");
    let w2 = port_value(this.rover, "drive_w2", "out");
    let w3 = port_value(this.rover, "drive_w3", "out");
    let w4 = port_value(this.rover, "drive_w4", "out");
    let w5 = port_value(this.rover, "drive_w5", "out");
    let travel = vlen(vsub(world_pos(this.rover), this.start));
    brake(this.rover);
    let checks = [
        t_present(w0, "drive_w0 output"),
        t_present(w1, "drive_w1 output"),
        t_present(w2, "drive_w2 output"),
        t_present(w3, "drive_w3 output"),
        t_present(w4, "drive_w4 output"),
        t_present(w5, "drive_w5 output"),
        t_true(this.asset_contract,
            "six-wheel independent asset did not expose six composed raycast wheels " +
            "with the shared tire/suspension parameters"),
        t_true(travel > 0.5, "six-wheel authored controller did not move the rover"),
    ];
    print("[six-wheel] travel=" + r2(travel) + "m");
    this.done = true;
    report_verdict(checks, "SIX-WHEEL CONTROLLER", "SIX_WHEEL_CONTROLLER");
}

fn on_stop(me) {
    if this.rover >= 0 { brake(this.rover); }
}
