// Authored acceptance test for the Ackermann rover.
//
// The scenario owns the behavioural assertion. Rust only projects the generic
// wheel/joint graph; the composed Modelica controller publishes drive values and
// final front-knuckle headings through the same port fabric used by this test.

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
    this.raycast = find("/AckermannParity/RoverRaycast");
    this.physical = find("/AckermannParity/RoverPhysical");
    this.raycast_start = ();
    this.physical_start = ();
    if this.raycast < 0 || this.physical < 0 {
        this.done = true;
        report_verdict(["Ackermann rover instances did not compose"],
                       "ACKERMANN CONTROLLER", "ACKERMANN_CONTROLLER");
        return;
    }
    this.raycast_start = world_pos(this.raycast);
    this.physical_start = world_pos(this.physical);
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 1.0 {
        brake(this.raycast);
        brake(this.physical);
        return;
    }

    // The high-level command remains generic. The composed controller chooses
    // which motor and joint ports receive it.
    drive(this.raycast, 1.0, 0.0);
    drive(this.physical, 1.0, 0.0);

    if this.elapsed < 8.0 { return; }
    brake(this.raycast);
    brake(this.physical);
    let rc_left = port_value(this.raycast, "drive_left", "out");
    let rc_right = port_value(this.raycast, "drive_right", "out");
    let ph_left = port_value(this.physical, "drive_left", "out");
    let ph_right = port_value(this.physical, "drive_right", "out");
    let rc_delta = vlen(vsub(world_pos(this.raycast), this.raycast_start));
    let ph_delta = vlen(vsub(world_pos(this.physical), this.physical_start));
    let checks = [
        t_present(rc_left, "raycast Modelica drive_left output"),
        t_present(rc_right, "raycast Modelica drive_right output"),
        t_present(ph_left, "physical Modelica drive_left output"),
        t_present(ph_right, "physical Modelica drive_right output"),
        t_true(rc_delta > 0.5, "raycast Ackermann rover did not move"),
        t_true(ph_delta > 0.5, "physical Ackermann rover did not move"),
    ];
    print("[ackermann] raycast travel=" + r2(rc_delta) +
          "m physical travel=" + r2(ph_delta) + "m");
    this.done = true;
    report_verdict(checks, "ACKERMANN CONTROLLER", "ACKERMANN_CONTROLLER");
}

fn on_stop(me) {
    if this.raycast >= 0 { brake(this.raycast); }
    if this.physical >= 0 { brake(this.physical); }
}
