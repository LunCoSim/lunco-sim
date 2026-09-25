// Authored behavioural acceptance for physical rover command transitions.
//
// The script owns only the sequence of generic commands and its observations.
// Modelica owns allocation and actuator math; USD owns the physical topology;
// Rust realizes the generic ports, contacts, joints, and body state.

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [
            find_path("/PhysicalRoverControls/SkidPhysical"),
            find_path("/PhysicalRoverControls/AckermannPhysical"),
        ],
        required_inputs: [],
    }
}

fn port_value(target, name, direction) {
    let record = query("ReadPorts", #{ api_id: target });
    if record == () || record.ports == () { return (); }
    for port in record.ports {
        if port.name == name && port.direction == direction { return port.value; }
    }
    ()
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.phase = "settle";
    this.phase_start = 0.0;
    this.active_is_skid = true;
    this.skid = find_path("/PhysicalRoverControls/SkidPhysical");
    this.ack = find_path("/PhysicalRoverControls/AckermannPhysical");
    this.jfl = find_path("/PhysicalRoverControls/AckermannPhysical/Steering_FL_Hinge");
    this.jfr = find_path("/PhysicalRoverControls/AckermannPhysical/Steering_FR_Hinge");
    this.kfl = find_path("/PhysicalRoverControls/AckermannPhysical/SteeringKnuckle_FL");
    this.kfr = find_path("/PhysicalRoverControls/AckermannPhysical/SteeringKnuckle_FR");
    this.wfl = find_path("/PhysicalRoverControls/AckermannPhysical/Suspension_FL/Wheel_FL");
    this.wfr = find_path("/PhysicalRoverControls/AckermannPhysical/Suspension_FR/Wheel_FR");
    this.prev_s = ();
    this.prev_a = ();
    this.fprev_s = ();
    this.fprev_a = ();
    this.reverse_f_s = ();
    this.reverse_f_a = ();

    this.forward_path_s = 0.0;
    this.forward_path_a = 0.0;
    this.right_yaw_s = 0.0;
    this.right_yaw_a = 0.0;
    this.left_yaw_s = 0.0;
    this.left_yaw_a = 0.0;
    this.reverse_projection_s = 0.0;
    this.reverse_projection_a = 0.0;
    this.samples = 0;
    this.max_tilt_s = 0.0;
    this.max_tilt_a = 0.0;
    this.settle_y_min_s = ();
    this.settle_y_max_s = ();
    this.settle_y_min_a = ();
    this.settle_y_max_a = ();
    this.brake_speed_s = ();
    this.brake_speed_a = ();
    this.forward_ack_s = ();
    this.forward_ack_a = ();
    this.forward_throttle_s = ();
    this.forward_throttle_a = ();
    this.forward_drive_left_s = ();
    this.forward_drive_right_s = ();
    this.forward_drive_left_a = ();
    this.forward_drive_right_a = ();
    this.forward_piloted_s = ();
    this.forward_piloted_a = ();
    this.skid_possession = ();
    this.ack_possession = ();
    this.settle_measurement_done = false;
    this.invalid_port_ack = ();
    this.invalid_port_throttle = ();
    if this.skid >= 0 {
        this.skid_possession = cmd("AcquireControl", #{
            target: this.skid,
            bind_camera: false,
        });
        expect_fault("missing_control_port");
        this.invalid_port_ack = cmd("SetPorts", #{
            target: this.skid,
            writes: [["throttle", 0.25], ["missing_control_port", 1.0]],
        });
        this.invalid_port_throttle = port_value(this.skid, "throttle", "in");
    }
    print("[physical-controls] skid=" + this.skid + " ackermann=" + this.ack +
          " steering_fl=" + this.jfl + " steering_fr=" + this.jfr);
    print("[physical-controls] claim skid -> settle -> skid matrix -> claim Ackermann -> Ackermann matrix");
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }
    if this.skid == () || this.skid < 0 || this.ack == () || this.ack < 0 {
        fail_fast("physical rover prims were not composed", "PHYSICAL ROVER CONTROLS", "PHYSICAL_ROVER_CONTROLS");
        this.phase = "done";
        return;
    }

    this.t += dt();
    let step = dt();
    let ps = world_pos(this.skid);
    let pa = world_pos(this.ack);
    let fs = world_forward(this.skid);
    let fa = world_forward(this.ack);
    if ps == () || pa == () || fs == () || fa == () {
        fail_fast("physical rover pose disappeared during the matrix", "PHYSICAL ROVER CONTROLS", "PHYSICAL_ROVER_CONTROLS");
        this.phase = "done";
        return;
    }

    let ds = seg(this.prev_s, ps);
    let da = seg(this.prev_a, pa);
    let previous_s = this.prev_s;
    let previous_a = this.prev_a;
    let vs = if step > 0.0 { ds / step } else { 0.0 };
    let va = if step > 0.0 { da / step } else { 0.0 };
    this.prev_s = ps;
    this.prev_a = pa;
    this.samples += 1;

    let ts = tilt_deg(this.skid);
    let ta = tilt_deg(this.ack);
    if ts >= 0.0 && ts > this.max_tilt_s { this.max_tilt_s = ts; }
    if ta >= 0.0 && ta > this.max_tilt_a { this.max_tilt_a = ta; }

    if this.phase == "settle" {
        brake(this.skid);
        brake(this.ack);
        // A post-contact window is the jitter observation. It starts after the
        // initial drop, so the measurement is suspension ring-down, not spawn.
        if !this.settle_measurement_done && this.t > 1.0 {
            if this.settle_y_min_s == () || ps[1] < this.settle_y_min_s { this.settle_y_min_s = ps[1]; }
            if this.settle_y_max_s == () || ps[1] > this.settle_y_max_s { this.settle_y_max_s = ps[1]; }
            if this.settle_y_min_a == () || pa[1] < this.settle_y_min_a { this.settle_y_min_a = pa[1]; }
            if this.settle_y_max_a == () || pa[1] > this.settle_y_max_a { this.settle_y_max_a = pa[1]; }
        }
        if this.t - this.phase_start >= 3.0 {
            this.settle_measurement_done = true;
            this.phase = "forward";
            this.phase_start = this.t;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] settled; forward command started");
        }
        return;
    }

    let elapsed = this.t - this.phase_start;
    if this.phase == "forward" {
        let target = if this.active_is_skid { this.skid } else { this.ack };
        let command = drive(target, 0.65, 0.0);
        if this.active_is_skid {
            if this.forward_ack_s == () { this.forward_ack_s = command; }
            this.forward_path_s += ds;
        } else {
            if this.forward_ack_a == () { this.forward_ack_a = command; }
            this.forward_path_a += da;
        }
        if elapsed >= 3.0 {
            if this.active_is_skid {
                this.forward_throttle_s = port_value(this.skid, "throttle", "in");
                this.forward_drive_left_s = port_value(this.skid, "drive_left", "out");
                this.forward_drive_right_s = port_value(this.skid, "drive_right", "out");
                this.forward_piloted_s = port_value(this.skid, "piloted", "out");
                print("[physical-controls] skid forward command=" + this.forward_ack_s +
                      " piloted=" + r2(this.forward_piloted_s) +
                      " throttle=" + r2(this.forward_throttle_s) +
                      " drive=" + r2(this.forward_drive_left_s) + "/" + r2(this.forward_drive_right_s));
            } else {
                this.forward_throttle_a = port_value(this.ack, "throttle", "in");
                this.forward_drive_left_a = port_value(this.ack, "drive_left", "out");
                this.forward_drive_right_a = port_value(this.ack, "drive_right", "out");
                this.forward_piloted_a = port_value(this.ack, "piloted", "out");
                print("[physical-controls] Ackermann forward command=" + this.forward_ack_a +
                      " piloted=" + r2(this.forward_piloted_a) +
                      " throttle=" + r2(this.forward_throttle_a) +
                      " drive=" + r2(this.forward_drive_left_a) + "/" + r2(this.forward_drive_right_a));
            }
            this.phase = "right";
            this.phase_start = this.t;
            if this.active_is_skid { this.right_yaw_s = 0.0; }
            else { this.right_yaw_a = 0.0; }
            this.fprev_s = fs;
            this.fprev_a = fa;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] steer-right command started");
        }
        return;
    }

    if this.phase == "right" {
        let target = if this.active_is_skid { this.skid } else { this.ack };
        drive(target, 0.65, 0.6);
        if this.active_is_skid {
            let dy_s = yaw_delta_deg(this.fprev_s, fs);
            if dy_s != () { this.right_yaw_s += dy_s; }
        } else {
            let dy_a = yaw_delta_deg(this.fprev_a, fa);
            if dy_a != () { this.right_yaw_a += dy_a; }
        }
        this.fprev_s = fs;
        this.fprev_a = fa;
        if elapsed >= 4.0 {
            if !this.active_is_skid {
                print("[physical-controls] Ackermann right headings fl=" + r2(get(this.ack, "heading_fl")) +
                      " fr=" + r2(get(this.ack, "heading_fr")) +
                      " joints fl=" + r2(get(this.jfl, "angle")) +
                      " fr=" + r2(get(this.jfr, "angle")) +
                      " wheel_contact fl=" + r2(get(this.wfl, "contact_force")) +
                      " fr=" + r2(get(this.wfr, "contact_force")));
            }
            this.phase = "left";
            this.phase_start = this.t;
            if this.active_is_skid { this.left_yaw_s = 0.0; }
            else { this.left_yaw_a = 0.0; }
            this.fprev_s = fs;
            this.fprev_a = fa;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] steer-left command started");
        }
        return;
    }

    if this.phase == "left" {
        let target = if this.active_is_skid { this.skid } else { this.ack };
        drive(target, 0.65, -0.6);
        if this.active_is_skid {
            let dy_s = yaw_delta_deg(this.fprev_s, fs);
            if dy_s != () { this.left_yaw_s += dy_s; }
        } else {
            let dy_a = yaw_delta_deg(this.fprev_a, fa);
            if dy_a != () { this.left_yaw_a += dy_a; }
        }
        this.fprev_s = fs;
        this.fprev_a = fa;
        if elapsed >= 4.0 {
            if !this.active_is_skid {
                print("[physical-controls] Ackermann left headings fl=" + r2(get(this.ack, "heading_fl")) +
                      " fr=" + r2(get(this.ack, "heading_fr")) +
                      " joints fl=" + r2(get(this.jfl, "angle")) +
                      " fr=" + r2(get(this.jfr, "angle")) +
                      " wheel_contact fl=" + r2(get(this.wfl, "contact_force")) +
                      " fr=" + r2(get(this.wfr, "contact_force")));
            }
            this.phase = "reverse";
            this.phase_start = this.t;
            if this.active_is_skid {
                this.reverse_f_s = fs;
                this.reverse_projection_s = 0.0;
            } else {
                this.reverse_f_a = fa;
                this.reverse_projection_a = 0.0;
            }
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] reverse command started");
        }
        return;
    }

    if this.phase == "reverse" {
        let target = if this.active_is_skid { this.skid } else { this.ack };
        drive(target, -0.5, 0.0);
        if this.active_is_skid {
            this.reverse_projection_s += vdot(vsub(ps, previous_s), this.reverse_f_s);
        } else {
            this.reverse_projection_a += vdot(vsub(pa, previous_a), this.reverse_f_a);
        }
        if elapsed >= 4.0 {
            this.phase = "brake";
            this.phase_start = this.t;
            if this.active_is_skid { this.brake_speed_s = vs; }
            else { this.brake_speed_a = va; }
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] brake command started");
        }
        return;
    }

    let target = if this.active_is_skid { this.skid } else { this.ack };
    brake(target);
    if this.active_is_skid { this.brake_speed_s = vs; }
    else { this.brake_speed_a = va; }
    if elapsed >= 2.0 {
        if this.active_is_skid {
            this.ack_possession = cmd("AcquireControl", #{
                target: this.ack,
                bind_camera: false,
            });
            this.active_is_skid = false;
            this.phase = "forward";
            this.phase_start = this.t;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] skid matrix complete; Ackermann control claimed");
            return;
        }
        this.phase = "done";
        let settle_span_s = this.settle_y_max_s - this.settle_y_min_s;
        let settle_span_a = this.settle_y_max_a - this.settle_y_min_a;
        let fails = [];
        fails.push(t_bounded(ps, 500.0, "physical skid chassis"));
        fails.push(t_bounded(pa, 500.0, "physical Ackermann chassis"));
        fails.push(t_true(this.forward_ack_s != () && this.forward_ack_s.ok == true,
            "physical skid SetPorts was not applied: " + this.forward_ack_s));
        fails.push(t_true(this.forward_ack_a != () && this.forward_ack_a.ok == true,
            "physical Ackermann SetPorts was not applied: " + this.forward_ack_a));
        fails.push(t_true(this.forward_ack_s.status == "applied",
            "physical skid SetPorts did not return a terminal applied result: " + this.forward_ack_s));
        fails.push(t_true(this.forward_ack_a.status == "applied",
            "physical Ackermann SetPorts did not return a terminal applied result: " + this.forward_ack_a));
        fails.push(t_true(this.invalid_port_ack != () && this.invalid_port_ack.status == "rejected"
                          && this.invalid_port_ack.ok == false,
            "invalid SetPorts batch did not return a terminal rejection: " + this.invalid_port_ack));
        fails.push(t_true(this.invalid_port_throttle == 0.0,
            "invalid SetPorts batch partially changed throttle to " + this.invalid_port_throttle));
        fails.push(t_true(this.skid_possession != () && this.skid_possession.ok == true,
            "physical skid rover was not claimed: " + this.skid_possession));
        fails.push(t_true(this.ack_possession != () && this.ack_possession.ok == true,
            "physical Ackermann rover was not claimed: " + this.ack_possession));
        fails.push(t_true(this.forward_throttle_s != () && (this.forward_throttle_s - 0.65).abs() < 0.01,
            "physical skid throttle input was " + this.forward_throttle_s));
        fails.push(t_true(this.forward_throttle_a != () && (this.forward_throttle_a - 0.65).abs() < 0.01,
            "physical Ackermann throttle input was " + this.forward_throttle_a));
        fails.push(t_true(this.forward_piloted_s == 1.0,
            "physical skid rover did not own control during its drive matrix"));
        fails.push(t_true(this.forward_piloted_a == 1.0,
            "physical Ackermann rover did not own control during its drive matrix"));
        fails.push(t_true(this.forward_drive_left_s != () && this.forward_drive_left_s > 0.1
                          && this.forward_drive_right_s > 0.1,
            "physical skid Modelica drive outputs were " + this.forward_drive_left_s + "/" + this.forward_drive_right_s));
        fails.push(t_true(this.forward_drive_left_a != () && this.forward_drive_left_a > 0.1
                          && this.forward_drive_right_a > 0.1,
            "physical Ackermann Modelica drive outputs were " + this.forward_drive_left_a + "/" + this.forward_drive_right_a));
        fails.push(t_moved(this.forward_path_s, 1.0, "physical skid forward command"));
        fails.push(t_moved(this.forward_path_a, 1.0, "physical Ackermann forward command"));
        fails.push(t_true(this.right_yaw_s.abs() > 2.0,
            "physical skid steer-right yaw was only " + r2(this.right_yaw_s) + " deg"));
        fails.push(t_true(this.right_yaw_a.abs() > 2.0,
            "physical Ackermann steer-right yaw was only " + r2(this.right_yaw_a) + " deg"));
        fails.push(t_true(this.left_yaw_s.abs() > 2.0,
            "physical skid steer-left yaw was only " + r2(this.left_yaw_s) + " deg"));
        fails.push(t_true(this.left_yaw_a.abs() > 2.0,
            "physical Ackermann steer-left yaw was only " + r2(this.left_yaw_a) + " deg"));
        fails.push(t_true(this.right_yaw_s * this.left_yaw_s < 0.0,
            "physical skid left/right commands did not produce opposite yaw signs"));
        fails.push(t_true(this.right_yaw_a * this.left_yaw_a < 0.0,
            "physical Ackermann left/right commands did not produce opposite yaw signs"));
        fails.push(t_true(this.reverse_projection_s < -0.1,
            "physical skid reverse command moved forward by " + r2(this.reverse_projection_s) + " m"));
        fails.push(t_true(this.reverse_projection_a < -0.1,
            "physical Ackermann reverse command moved forward by " + r2(this.reverse_projection_a) + " m"));
        fails.push(t_max(this.max_tilt_s, 20.0, "physical skid tilt"));
        fails.push(t_max(this.max_tilt_a, 20.0, "physical Ackermann tilt"));
        fails.push(t_max(settle_span_s, 0.25, "physical skid post-contact vertical jitter"));
        fails.push(t_max(settle_span_a, 0.25, "physical Ackermann post-contact vertical jitter"));
        fails.push(t_max(this.brake_speed_s, 0.75, "physical skid speed after braking"));
        fails.push(t_max(this.brake_speed_a, 0.75, "physical Ackermann speed after braking"));
        fails.push(t_true(this.samples > 600, "matrix produced only " + this.samples + " samples"));
        print("[physical-controls] forward path skid=" + r2(this.forward_path_s) +
              "m ackermann=" + r2(this.forward_path_a) + "m");
        print("[physical-controls] right yaw skid=" + r2(this.right_yaw_s) +
              "deg ackermann=" + r2(this.right_yaw_a) + "deg");
        print("[physical-controls] left yaw skid=" + r2(this.left_yaw_s) +
              "deg ackermann=" + r2(this.left_yaw_a) + "deg");
        print("[physical-controls] reverse projection skid=" + r2(this.reverse_projection_s) +
              "m ackermann=" + r2(this.reverse_projection_a) + "m");
        print("[physical-controls] settle vertical span skid=" + r2(settle_span_s) +
              "m ackermann=" + r2(settle_span_a) + "m");
        print("[physical-controls] max tilt skid=" + r2(this.max_tilt_s) +
              "deg ackermann=" + r2(this.max_tilt_a) + "deg");
        print("[physical-controls] speed after brake skid=" + r2(this.brake_speed_s) +
              "m/s ackermann=" + r2(this.brake_speed_a) + "m/s");
        report_verdict(fails, "PHYSICAL ROVER CONTROLS", "PHYSICAL_ROVER_CONTROLS");
    }
}

fn on_stop(me, ctx) {
    let skid = find_path("/PhysicalRoverControls/SkidPhysical");
    let ack = find_path("/PhysicalRoverControls/AckermannPhysical");
    if skid >= 0 { brake(skid); }
    if ack >= 0 { brake(ack); }
}
