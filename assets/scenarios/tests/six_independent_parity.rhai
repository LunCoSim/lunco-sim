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

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.rover = find("/SixIndependentParity/Rover");
    this.start = ();
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
    if this.elapsed < 8.0 { return; }

    let w0 = port_value(this.rover, "drive_w0", "Out");
    let w1 = port_value(this.rover, "drive_w1", "Out");
    let w2 = port_value(this.rover, "drive_w2", "Out");
    let w3 = port_value(this.rover, "drive_w3", "Out");
    let w4 = port_value(this.rover, "drive_w4", "Out");
    let w5 = port_value(this.rover, "drive_w5", "Out");
    let travel = vlen(vsub(world_pos(this.rover), this.start));
    brake(this.rover);
    let checks = [
        t_present(w0, "drive_w0 output"),
        t_present(w1, "drive_w1 output"),
        t_present(w2, "drive_w2 output"),
        t_present(w3, "drive_w3 output"),
        t_present(w4, "drive_w4 output"),
        t_present(w5, "drive_w5 output"),
        t_true(travel > 0.5, "six-wheel authored controller did not move the rover"),
    ];
    print("[six-wheel] travel=" + r2(travel) + "m");
    this.done = true;
    report_verdict(checks, "SIX-WHEEL CONTROLLER", "SIX_WHEEL_CONTROLLER");
}

fn on_stop(me) {
    if this.rover >= 0 { brake(this.rover); }
}
