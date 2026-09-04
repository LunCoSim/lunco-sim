// Authored behavioural acceptance for the USD/Modelica/Rhai robotic hand.
//
// USD owns the links, joints, limits and collision shapes. Modelica owns the
// filtered closure state and symmetric joint-angle math. Rhai owns this
// command sequence and its observations; it does not calculate joint motion.

fn issue_hand_command(state, open_value, grasp_value) {
    let result = cmd("SetPorts", #{
        target: state.controller,
        writes: [["open_command", open_value], ["grasp_command", grasp_value]],
    });
    result.ok
}

fn on_start(me) {
    this.t = 0.0;
    this.phase = "await_ports";
    this.phase_start = 0.0;
    this.readiness_timeout = 12.0;
    this.controller = -1;
    this.left_joint = -1;
    this.center_joint = -1;
    this.right_joint = -1;
    this.palm = -1;
    this.left = -1;
    this.center = -1;
    this.right = -1;
    this.target = -1;
    this.command_ok = false;
    this.open_closure = ();
    this.close_closure = ();
    this.half_closure = ();
    this.release_closure = ();
    this.close_left = ();
    this.close_center = ();
    this.close_right = ();
    this.close_target_contact = ();
    this.max_target_contact = 0.0;
    this.samples = 0;

    print("[hand] waiting for authored topology and Modelica ports");
}

fn resolve_handles() {
    #{
        controller: find("/RoboticHandControls/Hand/Controller"),
        left_joint: find("/RoboticHandControls/Hand/Finger_Left_Hinge"),
        center_joint: find("/RoboticHandControls/Hand/Finger_Center_Hinge"),
        right_joint: find("/RoboticHandControls/Hand/Finger_Right_Hinge"),
        palm: find("/RoboticHandControls/Hand/Palm"),
        left: find("/RoboticHandControls/Hand/Finger_Left"),
        center: find("/RoboticHandControls/Hand/Finger_Center"),
        right: find("/RoboticHandControls/Hand/Finger_Right"),
        target: find("/RoboticHandControls/Hand/GripTarget"),
    }
}

fn topology_ready(state) {
    state.controller >= 0 && state.left_joint >= 0 && state.center_joint >= 0 &&
    state.right_joint >= 0 && state.palm >= 0 && state.left >= 0 &&
    state.center >= 0 && state.right >= 0 && state.target >= 0 &&
    get(state.controller, "closure") != () &&
    get(state.controller, "left_angle") != () &&
    get(state.controller, "right_angle") != ()
}

fn bounded_pose(id, label) {
    t_bounded(world_pos(id), 100.0, label)
}

fn on_tick(me) {
    if this.phase == "done" { return; }
    this.t += dt();
    let handles = resolve_handles();
    this.controller = handles.controller;
    this.left_joint = handles.left_joint;
    this.center_joint = handles.center_joint;
    this.right_joint = handles.right_joint;
    this.palm = handles.palm;
    this.left = handles.left;
    this.center = handles.center;
    this.right = handles.right;
    this.target = handles.target;

    if this.phase == "await_ports" {
        if topology_ready(this) {
            this.command_ok = issue_hand_command(this, 1.0, 0.0);
            if !this.command_ok {
                fail_fast("generic SetPorts could not reach the authored hand controller",
                          "ROBOTIC HAND", "ROBOTIC_HAND_CONTROLS");
                this.phase = "done";
                return;
            }
            this.phase = "open";
            this.phase_start = this.t;
            print("[hand] OPEN: open_command=1 grasp_command=0");
        } else if this.t >= this.readiness_timeout {
            let ports = query("ReadPorts", #{ api_id: this.controller });
            print("[hand] readiness ids controller=" + this.controller +
                  " left_joint=" + this.left_joint + " center_joint=" + this.center_joint +
                  " right_joint=" + this.right_joint + " palm=" + this.palm +
                  " left=" + this.left + " center=" + this.center +
                  " right=" + this.right + " target=" + this.target +
                  " ports=" + ports);
            fail_fast("authored hand topology or Modelica ports did not materialize within " +
                      r2(this.readiness_timeout) + " s", "ROBOTIC HAND", "ROBOTIC_HAND_CONTROLS");
            this.phase = "done";
        }
        return;
    }

    this.samples += 1;
    let contact = get(this.target, "contact_force");
    if contact != () && contact > this.max_target_contact { this.max_target_contact = contact; }

    if this.phase == "open" {
        if this.t - this.phase_start >= 2.0 {
            this.open_closure = get(this.controller, "closure");
            this.phase = "close";
            this.phase_start = this.t;
            this.command_ok = issue_hand_command(this, 0.0, 1.0) && this.command_ok;
            print("[hand] CLOSE: open_command=0 grasp_command=1");
        }
        return;
    }

    if this.phase == "close" {
        if this.t - this.phase_start >= 3.0 {
            this.close_closure = get(this.controller, "closure");
            this.close_left = get(this.left_joint, "angle");
            this.close_center = get(this.center_joint, "angle");
            this.close_right = get(this.right_joint, "angle");
            this.close_target_contact = contact;
            this.phase = "half";
            this.phase_start = this.t;
            this.command_ok = issue_hand_command(this, 0.25, 0.75) && this.command_ok;
            print("[hand] HALF: open_command=0.25 grasp_command=0.75");
        }
        return;
    }

    if this.phase == "half" {
        if this.t - this.phase_start >= 2.0 {
            this.half_closure = get(this.controller, "closure");
            this.phase = "release";
            this.phase_start = this.t;
            this.command_ok = issue_hand_command(this, 1.0, 0.0) && this.command_ok;
            print("[hand] RELEASE: open_command=1 grasp_command=0");
        }
        return;
    }

    if this.t - this.phase_start < 2.0 { return; }
    this.release_closure = get(this.controller, "closure");
    this.phase = "done";

    let failures = [];
    failures.push(t_true(this.command_ok, "hand command sequence did not reach every phase"));
    failures.push(t_present(this.open_closure, "Modelica open closure"));
    failures.push(t_present(this.close_closure, "Modelica close closure"));
    failures.push(t_present(this.half_closure, "Modelica half closure"));
    failures.push(t_present(this.release_closure, "Modelica release closure"));
    failures.push(t_range(this.open_closure, 0.0, 0.1, "open closure"));
    failures.push(t_range(this.close_closure, 0.65, 1.0, "close closure"));
    failures.push(t_range(this.half_closure, 0.2, 0.8, "half closure"));
    failures.push(t_range(this.release_closure, 0.0, 0.1, "release closure"));
    failures.push(t_present(this.close_left, "left joint angle"));
    failures.push(t_present(this.close_center, "center joint angle"));
    failures.push(t_present(this.close_right, "right joint angle"));
    failures.push(t_present(this.close_target_contact, "grip target contact force"));
    if this.close_left != () {
        failures.push(t_true(this.close_left > 0.25,
                             "left finger did not close toward the target"));
    }
    if this.close_right != () {
        failures.push(t_true(this.close_right < -0.25,
                             "right finger did not close toward the target"));
    }
    if this.close_target_contact != () {
        failures.push(t_true(this.close_target_contact > 0.01,
                             "closing fingers never made physical contact with the authored target"));
    }
    failures.push(bounded_pose(this.palm, "hand palm"));
    failures.push(bounded_pose(this.left, "left finger"));
    failures.push(bounded_pose(this.center, "center finger"));
    failures.push(bounded_pose(this.right, "right finger"));
    failures.push(bounded_pose(this.target, "hand grip target"));
    failures.push(t_true(this.samples > 400, "hand matrix produced too few samples"));

    print("[hand] open=" + r4(this.open_closure) +
          " close=" + r4(this.close_closure) +
          " half=" + r4(this.half_closure) +
          " release=" + r4(this.release_closure));
    print("[hand] close joint angles left=" + r4(this.close_left) +
          " center=" + r4(this.close_center) +
          " right=" + r4(this.close_right) +
          " target_contact=" + r4(this.close_target_contact) +
          " max_contact=" + r4(this.max_target_contact));
    report_verdict(failures, "ROBOTIC HAND", "ROBOTIC_HAND_CONTROLS");
}

fn on_stop(me) {
    if this.controller >= 0 {
        cmd("SetPorts", #{
            target: this.controller,
            writes: [["open_command", 1.0], ["grasp_command", 0.0]],
        });
    }
}
