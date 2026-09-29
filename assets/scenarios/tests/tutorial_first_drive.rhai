//! Regression harness for the built-in First Drive lesson.
//!
//! The lesson's own `task(me)` is the subject under test. This scenario only
//! observes its public command/event surface and reports a verdict when the
//! lesson's real mission completes. It intentionally does not drive the rover
//! itself: duplicating the autopilot here would allow the test to pass while
//! the built-in lesson was broken.

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

fn on_start(me, ctx) {
    this.port_writes = 0;
    this.verdict_done = false;
    this.release_requested = false;
    this.release_seen = false;
    this.safe_stop_seen = false;
    this.release_tick = -1;
    this.release_elapsed = 0.0;
    this.elapsed = 0.0;
    this.sun = find_path("/FirstDrive/Sun");
    this.rover = find_path("/FirstDrive/Rover");
    this.flag_sensor = find_path("/FirstDrive/Flag/Trigger");
    this.waypoint_event_seen = false;
    this.rover_entered_waypoint = false;
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
    } else if evt.name == "MISSION_FAILED" {
        if !this.verdict_done {
            this.verdict_done = true;
            verdict(false, "the built-in First Drive mission failed");
        }
    } else if evt.name == "cmd:ReleaseControlSource" && this.release_requested {
        this.release_seen = true;
    } else if evt.name == "cmd:ReleaseControl" && this.release_requested {
        this.safe_stop_seen = true;
        this.release_tick = sim_tick();
    } else if evt.name == "MISSION_COMPLETE" {
        let guidance_used = this.port_writes > 0;
        let sun_forward = world_forward(this.sun);
        let light_is_downward = this.sun >= 0 && sun_forward != () && sun_forward[1] < -0.25;
        if !light_is_downward {
            this.verdict_done = true;
            verdict(false, "the authored scene sun is not emitting down onto the surface");
        } else if !guidance_used {
            this.verdict_done = true;
            verdict(false, "the authored task did not publish Modelica guidance inputs");
        } else if !this.waypoint_event_seen || !this.rover_entered_waypoint {
            this.verdict_done = true;
            verdict(false, "mission completed without this rover entering the flag's Avian waypoint sensor");
        } else {
            // Exercise the learner's release path after the built-in mission
            // completes, then inspect the rover's live physics state.
            this.release_requested = true;
            cmd("ReleaseControlSource", #{ source: find_path("/FirstDrive/Avatar") });
        }
    }
}

// A real verdict is required even when the lesson stalls. This is test-only
// liveness handling; the lesson itself remains event/state driven.
fn on_tick(me, ctx) {
    if this.verdict_done { return; }
    this.elapsed += dt();
    if this.safe_stop_seen && sim_tick() > this.release_tick {
        let rover = find_path("/FirstDrive/Rover");
        if rover < 0 {
            this.verdict_done = true;
            verdict(false, "the rover entity disappeared after possession was released");
            return;
        }
        let state = query("QueryPhysicsState", #{ id: rover });
        if state == () || state.ok == false {
            this.verdict_done = true;
            verdict(false, "the rover physics state became unavailable after possession was released");
            return;
        }
        let position = world_pos(rover);
        let inertia = state.inertia_principal_kgm2;
        let mass_valid = finite_positive(state.mass_kg);
        let inertia_valid = finite_vec3(inertia)
            && inertia[0] > 0.0 && inertia[1] > 0.0 && inertia[2] > 0.0;
        let brake = port(rover, "brake");
        let program_active = port(rover, "program_active");
        let body_valid = this.safe_stop_seen
            && brake != () && brake >= 0.99
            && program_active != () && program_active <= 0.01
            && state.usd_prim_path == "/FirstDrive/Rover"
            && state.body_mode == "dynamic"
            && state.physics_state_ready == true
            && state.physics_state_pending == false
            && state.physics_initialization_invalid != true
            && state.rigid_body_disabled != true
            && mass_valid && inertia_valid && finite_vec3(position);
        if !body_valid {
            this.verdict_done = true;
            print("TUTORIAL_FIRST_DRIVE release pose=" + position.to_string()
                + " velocity=" + state.linear_velocity_mps.to_string());
            print("TUTORIAL_FIRST_DRIVE release mass=" + state.mass_kg.to_string()
                + " inertia=" + inertia.to_string()
                + " collider=" + state.collider_present.to_string());
            print("TUTORIAL_FIRST_DRIVE release mode=" + state.body_mode.to_string()
                + " ready=" + state.physics_state_ready.to_string()
                + " pending=" + state.physics_state_pending.to_string()
                + " invalid=" + state.physics_initialization_invalid.to_string());
            print("TUTORIAL_FIRST_DRIVE release disabled=" + state.rigid_body_disabled.to_string()
                + " brake=" + brake.to_string()
                + " program_active=" + program_active.to_string());
            verdict(false, "the rover was not safely stopped or lost finite pose/mass properties after possession was released");
            return;
        }
        this.release_elapsed += dt();
        if this.release_elapsed >= 2.0 {
            this.verdict_done = true;
            verdict(true, "");
            return;
        }
    }
    if this.elapsed > 60.0 {
        let rover = find_path("/FirstDrive/Rover");
        let flag = find_path("/FirstDrive/Flag");
        print("TUTORIAL_FIRST_DRIVE state: rover=" + world_pos(rover).to_string()
            + " flag=" + world_pos(flag).to_string()
            + " program=" + port(rover, "program_active").to_string()
            + " guidance=" + port(rover, "guidance_throttle").to_string()
            + " brake=" + port(rover, "brake").to_string());
        print("TUTORIAL_FIRST_DRIVE timeout: set_ports=" + this.port_writes +
              " waypoint_event=" + this.waypoint_event_seen +
              " rover_entered=" + this.rover_entered_waypoint +
              " release=" + this.release_seen + " safe_stop=" + this.safe_stop_seen);
        this.verdict_done = true;
        if this.release_requested {
            verdict(false, "the rover was not safely stopped and valid after possession was released");
        } else {
            verdict(false, "lesson did not complete within 60 simulated seconds");
        }
    }
}
