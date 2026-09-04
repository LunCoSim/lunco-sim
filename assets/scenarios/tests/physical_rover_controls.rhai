// Authored behavioural acceptance for physical rover command transitions.
//
// The script owns only the sequence of generic commands and its observations.
// Modelica owns allocation and actuator math; USD owns the physical topology;
// Rust realizes the generic ports, contacts, joints, and body state.

fn on_start(me) {
    this.t = 0.0;
    this.phase = "settle";
    this.skid = find("/PhysicalRoverControls/SkidPhysical");
    this.ack = find("/PhysicalRoverControls/AckermannPhysical");
    this.jfl = find("/PhysicalRoverControls/AckermannPhysical/Steering_FL_Hinge");
    this.jfr = find("/PhysicalRoverControls/AckermannPhysical/Steering_FR_Hinge");
    this.kfl = find("/PhysicalRoverControls/AckermannPhysical/SteeringKnuckle_FL");
    this.kfr = find("/PhysicalRoverControls/AckermannPhysical/SteeringKnuckle_FR");
    this.wfl = find("/PhysicalRoverControls/AckermannPhysical/Suspension_FL/Wheel_FL");
    this.wfr = find("/PhysicalRoverControls/AckermannPhysical/Suspension_FR/Wheel_FR");
    this.prev_s = ();
    this.prev_a = ();
    this.fprev_s = ();
    this.fprev_a = ();
    this.right_f_s = ();
    this.right_f_a = ();
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
    print("[physical-controls] skid=" + this.skid + " ackermann=" + this.ack +
          " steering_fl=" + this.jfl + " steering_fr=" + this.jfr);
    print("[physical-controls] settle -> forward -> steer right -> steer left -> reverse -> brake");
}

fn on_tick(me) {
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
        if this.t > 1.0 {
            if this.settle_y_min_s == () || ps[1] < this.settle_y_min_s { this.settle_y_min_s = ps[1]; }
            if this.settle_y_max_s == () || ps[1] > this.settle_y_max_s { this.settle_y_max_s = ps[1]; }
            if this.settle_y_min_a == () || pa[1] < this.settle_y_min_a { this.settle_y_min_a = pa[1]; }
            if this.settle_y_max_a == () || pa[1] > this.settle_y_max_a { this.settle_y_max_a = pa[1]; }
        }
        if this.t >= 3.0 {
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
        drive(this.skid, 0.65, 0.0);
        drive(this.ack, 0.65, 0.0);
        this.forward_path_s += ds;
        this.forward_path_a += da;
        if elapsed >= 3.0 {
            this.phase = "right";
            this.phase_start = this.t;
            this.right_yaw_s = 0.0;
            this.right_yaw_a = 0.0;
            this.fprev_s = fs;
            this.fprev_a = fa;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] steer-right command started");
        }
        return;
    }

    if this.phase == "right" {
        drive(this.skid, 0.65, 0.6);
        drive(this.ack, 0.65, 0.6);
        let dy_s = yaw_delta_deg(this.fprev_s, fs);
        let dy_a = yaw_delta_deg(this.fprev_a, fa);
        if dy_s != () { this.right_yaw_s += dy_s; }
        if dy_a != () { this.right_yaw_a += dy_a; }
        this.fprev_s = fs;
        this.fprev_a = fa;
        if elapsed >= 4.0 {
            print("[physical-controls] right model headings fl=" + r2(get(this.ack, "heading_fl")) +
                  " fr=" + r2(get(this.ack, "heading_fr")) +
                  " joints fl=" + r2(get(this.jfl, "angle")) +
                  " fr=" + r2(get(this.jfr, "angle")) +
                  " knuckle_contact fl=" + r2(get(this.kfl, "contact_force")) +
                  " fr=" + r2(get(this.kfr, "contact_force")) +
                  " wheel_contact fl=" + r2(get(this.wfl, "contact_force")) +
                  " fr=" + r2(get(this.wfr, "contact_force")));
            this.phase = "left";
            this.phase_start = this.t;
            this.left_yaw_s = 0.0;
            this.left_yaw_a = 0.0;
            this.fprev_s = fs;
            this.fprev_a = fa;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] steer-left command started");
        }
        return;
    }

    if this.phase == "left" {
        drive(this.skid, 0.65, -0.6);
        drive(this.ack, 0.65, -0.6);
        let dy_s = yaw_delta_deg(this.fprev_s, fs);
        let dy_a = yaw_delta_deg(this.fprev_a, fa);
        if dy_s != () { this.left_yaw_s += dy_s; }
        if dy_a != () { this.left_yaw_a += dy_a; }
        this.fprev_s = fs;
        this.fprev_a = fa;
        if elapsed >= 4.0 {
            print("[physical-controls] left model headings fl=" + r2(get(this.ack, "heading_fl")) +
                  " fr=" + r2(get(this.ack, "heading_fr")) +
                  " joints fl=" + r2(get(this.jfl, "angle")) +
                  " fr=" + r2(get(this.jfr, "angle")) +
                  " knuckle_contact fl=" + r2(get(this.kfl, "contact_force")) +
                  " fr=" + r2(get(this.kfr, "contact_force")) +
                  " wheel_contact fl=" + r2(get(this.wfl, "contact_force")) +
                  " fr=" + r2(get(this.wfr, "contact_force")));
            this.phase = "reverse";
            this.phase_start = this.t;
            this.reverse_f_s = fs;
            this.reverse_f_a = fa;
            this.reverse_projection_s = 0.0;
            this.reverse_projection_a = 0.0;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] reverse command started");
        }
        return;
    }

    if this.phase == "reverse" {
        drive(this.skid, -0.5, 0.0);
        drive(this.ack, -0.5, 0.0);
        this.reverse_projection_s += vdot(vsub(ps, previous_s), this.reverse_f_s);
        this.reverse_projection_a += vdot(vsub(pa, previous_a), this.reverse_f_a);
        if elapsed >= 4.0 {
            this.phase = "brake";
            this.phase_start = this.t;
            this.brake_speed_s = vs;
            this.brake_speed_a = va;
            this.prev_s = ps;
            this.prev_a = pa;
            print("[physical-controls] brake command started");
        }
        return;
    }

    brake(this.skid);
    brake(this.ack);
    this.brake_speed_s = vs;
    this.brake_speed_a = va;
    if elapsed >= 2.0 {
        this.phase = "done";
        let settle_span_s = this.settle_y_max_s - this.settle_y_min_s;
        let settle_span_a = this.settle_y_max_a - this.settle_y_min_a;
        let fails = [];
        fails.push(t_bounded(ps, 500.0, "physical skid chassis"));
        fails.push(t_bounded(pa, 500.0, "physical Ackermann chassis"));
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

fn on_stop(me) {
    let skid = find("/PhysicalRoverControls/SkidPhysical");
    let ack = find("/PhysicalRoverControls/AckermannPhysical");
    if skid >= 0 { brake(skid); }
    if ack >= 0 { brake(ack); }
}
