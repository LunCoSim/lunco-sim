//! Production regression for the built-in First Drive lesson.
//!
//! The scenario claims and releases rover authority while the lesson's Modelica
//! waypoint guidance drives it. Release must clear controller state,
//! preserve the authored autopilot setpoints, and leave the rover's physics
//! state continuous until the waypoint sensor reports arrival.

fn simulation_dependencies(me, ctx) {
    let rover = find_path("/FirstDrive/Rover");
    let flag = find_path("/FirstDrive/Flag");
    let sensor = find_path("/FirstDrive/Flag/Trigger");
    let sun = find_path("/FirstDrive/Sun");
    if rover < 0 || flag < 0 || sensor < 0 || sun < 0 {
        throw "First Drive regression requires its authored rover, flag sensor, and sun";
    }
    #{ modelica_entities: [rover], entity_reads: [flag, sensor, sun],
       entity_writes: [], query_reads: ["QueryPhysicsState"], required_inputs: [] }
}

fn event_is_within(entity, ancestor) {
    if entity == () || ancestor < 0 { return false; }
    let current = entity.to_int();
    if current < 0 { return false; }
    for depth in 0..32 {
        if current == ancestor { return true; }
        let next = parent(current);
        if next == () || next < 0 { return false; }
        current = next;
    }
    false
}

fn finite_positive(value) {
    value != () && value.is_finite() && value > 0.0
}

fn finite_vec3(value) {
    value != () && value.len() == 3
        && value[0].is_finite() && value[1].is_finite() && value[2].is_finite()
}

fn valid_rover_physics(rover) {
    let state = query("QueryPhysicsState", #{ id: rover });
    let position = world_pos(rover);
    if state == () || state.ok == false { return false; }
    finite_positive(state.mass_kg)
        && finite_vec3(state.inertia_principal_kgm2)
        && state.inertia_principal_kgm2[0] > 0.0
        && state.inertia_principal_kgm2[1] > 0.0
        && state.inertia_principal_kgm2[2] > 0.0
        && state.usd_prim_path == "/FirstDrive/Rover"
        && state.body_mode == "dynamic"
        && state.physics_state_ready == true
        && state.physics_state_pending == false
        && state.physics_initialization_invalid != true
        && state.rigid_body_disabled != true
        && finite_vec3(position)
}

fn moved_since(start, current) {
    if !finite_vec3(start) || !finite_vec3(current) { return false; }
    let dx = current[0] - start[0];
    let dy = current[1] - start[1];
    let dz = current[2] - start[2];
    dx * dx + dy * dy + dz * dz > 0.01
}

fn on_start(me, ctx) {
    this.done = false;
    this.phase = 0;
    this.elapsed = 0.0;
    this.release_elapsed = 0.0;
    this.completion_elapsed = 0.0;
    this.physics_sample_elapsed = 0.0;
    this.post_arrival_elapsed = 0.0;
    this.port_writes = 0;
    this.rover = find_path("/FirstDrive/Rover");
    this.flag_sensor = find_path("/FirstDrive/Flag/Trigger");
    this.waypoint_event_seen = false;
    this.rover_entered_waypoint = false;
    this.release_requested = false;
    this.release_seen = false;
    this.release_command_seen = false;
    this.autopilot_after_release = false;
    this.moved_after_release = false;
    this.mission_complete = false;
    this.position_at_release = ();
    this.physics_checks = 0;

    let claim = cmd("ClaimControl", #{ target: this.rover });
    if claim == () || claim.ok != true {
        this.done = true;
        verdict(false, "the regression could not claim the rover's control authority");
        return;
    }
}

fn verdict(pass, reason) {
    if pass {
        emit("TUTORIAL_FIRST_DRIVE", "PASS");
    } else {
        print("TUTORIAL_FIRST_DRIVE FAIL: " + reason);
        emit("TUTORIAL_FIRST_DRIVE", "FAIL");
    }
}

fn on_event(me, evt, ctx) {
    if sensor_entered(evt, this.flag_sensor) {
        this.waypoint_event_seen = true;
        this.rover_entered_waypoint = event_is_within(evt.value, this.rover);
    }
    if evt.name == "cmd:SetPorts" {
        this.port_writes += 1;
    } else if evt.name == "cmd:ReleaseControlClaim" && this.release_requested {
        this.release_seen = true;
    } else if evt.name == "cmd:ReleaseControl" {
        this.release_command_seen = true;
    } else if evt.name == "MISSION_FAILED" {
        if !this.done {
            this.done = true;
            verdict(false, "the built-in First Drive mission failed");
        }
    } else if evt.name == "MISSION_COMPLETE" {
        this.mission_complete = true;
    }
}

// The lesson owns its unattended Modelica guidance. This regression claims and
// releases session authority while that guidance runs, then observes the public
// waypoint event and authored stop sequence.
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 60.0 {
        this.done = true;
        verdict(false, "the lesson did not complete within 60 simulated seconds");
        return;
    }

    let rover = find_path("/FirstDrive/Rover");
    if rover < 0 {
        this.done = true;
        verdict(false, "the rover entity disappeared during the lesson");
        return;
    }

    if this.phase == 0 {
        if is_controlled(rover)
            && port(rover, "program_active") >= 0.99
            && port(rover, "autopilot_enable") >= 0.99 {
            this.position_at_release = world_pos(rover);
            this.release_requested = true;
            let released = cmd("ReleaseControlClaim", #{ target: rover });
            if released == () || released.ok != true {
                this.done = true;
                verdict(false, "the regression could not release rover authority");
                return;
            }
            this.phase = 1;
            this.release_elapsed = 0.0;
        }
        return;
    }

    if this.phase == 1 {
        this.release_elapsed += dt();
        this.physics_sample_elapsed += dt();
        let position = world_pos(rover);
        if moved_since(this.position_at_release, position) {
            this.moved_after_release = true;
        }
        if port(rover, "program_active") >= 0.99
            && port(rover, "autopilot_enable") >= 0.99 {
            this.autopilot_after_release = true;
        }
        if this.physics_sample_elapsed >= 0.25 {
            this.physics_sample_elapsed = 0.0;
            if !valid_rover_physics(rover) {
                this.done = true;
                verdict(false, "rover physics state became invalid after control release");
                return;
            }
            this.physics_checks += 1;
        }
        if this.mission_complete {
            if !this.release_seen || this.release_command_seen {
                this.done = true;
                verdict(false, "control release was missing or the lesson issued an extra ReleaseControl command");
                return;
            }
            if !this.autopilot_after_release || !this.moved_after_release {
                this.done = true;
                verdict(false, "authored autopilot setpoints or rover motion stopped when authority was released");
                return;
            }
            if !this.waypoint_event_seen || !this.rover_entered_waypoint {
                this.done = true;
                verdict(false, "mission completed without this rover entering the Avian waypoint sensor");
                return;
            }
            if this.port_writes == 0 {
                this.done = true;
                verdict(false, "the authored task did not publish Modelica guidance inputs");
                return;
            }
            this.phase = 2;
            this.completion_elapsed = 0.0;
            this.physics_sample_elapsed = 0.0;
            return;
        }
        if this.release_elapsed > 30.0 {
            this.done = true;
            verdict(false, "the waypoint mission did not complete after possession release");
        }
        return;
    }

    // Arrival is the lesson's explicit stop point. Confirm that this authored
    // stop, which follows the Avian sensor event, leaves a valid dynamic body.
    this.completion_elapsed += dt();
    this.physics_sample_elapsed += dt();
    if this.physics_sample_elapsed >= 0.25 {
        this.physics_sample_elapsed = 0.0;
        if !valid_rover_physics(rover) {
            this.done = true;
            verdict(false, "the rover lost valid physics state after waypoint arrival");
            return;
        }
        this.physics_checks += 1;
    }
    if port(rover, "program_active") <= 0.01 && port(rover, "brake") >= 0.99 {
        this.post_arrival_elapsed += dt();
    }
    if this.post_arrival_elapsed >= 2.0 {
        this.done = true;
        if this.physics_checks < 8 {
            verdict(false, "the rover physics state was not sampled across release and arrival");
        } else {
            verdict(true, "");
        }
        return;
    }
    if this.completion_elapsed > 10.0 {
        this.done = true;
        verdict(false, "the lesson did not apply its authored stop after sensor arrival");
    }
}
