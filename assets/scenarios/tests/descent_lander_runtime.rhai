//! Production descent/lander runtime acceptance.
//!
//! This observes the authored `lander_ops.usda` mission in one process.  It
//! proves the real Modelica propulsion projection is live (thrust, propellant,
//! mass and inertia), the authored touchdown/deployment events occur, and the
//! production rover completes its route through the normal autopilot/control
//! boundary.  The negative handoff check uses only the public command surface;
//! it does not write transforms or physics state.

fn verdict(pass, reason) {
    if pass {
        emit("DESCENT_LANDER_RUNTIME", "PASS");
    } else {
        print("DESCENT_LANDER_RUNTIME FAIL: " + reason);
        emit("DESCENT_LANDER_RUNTIME", "FAIL");
    }
}

fn scalar_ok(v) {
    v != () && v.is_finite();
}

fn high(id, port) {
    if id < 0 { return false; }
    let value = get(id, port);
    scalar_ok(value) && value >= 0.5
}

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
    this.engine_cutoff = false;
    this.flight_handoff = false;
    this.touchdown = false;
    this.deployed = false;
    this.course_complete = false;
    this.main = find("/LanderTest/Lander/MainPropulsion");
    this.rcs = find("/LanderTest/Lander/AttitudePropulsion");
    this.lander = find("/LanderTest/Lander");
    this.gnc = find("/LanderTest/Lander/GNC");
    // Contact is owned by each Leg* rigid body. Pad* is only its welded
    // collider child and has no independent runtime contact surface.
    this.leg_px = find("/LanderTest/Lander/LegPX");
    this.leg_nx = find("/LanderTest/Lander/LegNX");
    this.leg_pz = find("/LanderTest/Lander/LegPZ");
    this.leg_nz = find("/LanderTest/Lander/LegNZ");
    this.rover = find("/LanderTest/SkidRover");
    this.goal = find("/LanderTest/RoverTarget3");
    this.event_sources_ok = true;
    this.native_contacts_seen = false;
    this.model_contacts_seen = false;
    this.initial_mass = ();
    this.final_mass = ();
    this.initial_propellant = ();
    this.final_propellant = ();
    this.max_thrust = 0.0;
    this.max_rcs_flow = 0.0;
    this.max_pre_handoff_torque = 0.0;
    this.post_handoff = false;
    this.post_handoff_elapsed = 0.0;
    this.post_handoff_quiet_elapsed = 0.0;
    this.max_post_handoff_torque = 0.0;
    this.max_post_handoff_throttle = 0.0;
    this.max_post_handoff_thrust = 0.0;
    this.max_post_handoff_rcs_flow = 0.0;
    this.post_handoff_initial_speed = ();
    this.post_handoff_initial_rate = ();
    this.post_handoff_final_speed = ();
    this.post_handoff_final_rate = ();
    this.post_handoff_landing_state = ();
    this.post_handoff_cutoff_state = ();
    this.stale_command_injected = false;
    this.stale_command_ok = true;
    print("[descent-runtime] lander=" + this.lander +
          " gnc=" + this.gnc + " main=" + this.main + " rcs=" + this.rcs +
          " rover=" + this.rover);
}

fn on_event(me, evt) {
    if this.done { return; }
    if evt.name == "lander_engine_cutoff" {
        if evt.source != this.gnc { this.event_sources_ok = false; }
        this.engine_cutoff = true;
    } else if evt.name == "lander_flight_handoff" {
        if evt.source != this.gnc { this.event_sources_ok = false; }
        this.flight_handoff = true;
        this.post_handoff = true;
        this.post_handoff_elapsed = 0.0;
    } else if evt.name == "lander_touchdown" {
        if evt.source != this.lander { this.event_sources_ok = false; }
        this.touchdown = true;
    } else if evt.name == "rover_deployed" {
        this.deployed = true;
        print("[descent-runtime] rover_deployed rover=" + world_pos(this.rover));
    } else if evt.name == "waypoint.reached" {
        print("[descent-runtime] waypoint_reached value=" + evt.value +
              " rover=" + world_pos(this.rover));
    } else if evt.name == "course_complete" {
        print("[descent-runtime] course_complete observed; finalizing acceptance");
        this.course_complete = true;
        this.final_mass = get(this.main, "vehicle_mass_kg");
        this.final_propellant = get(this.main, "propellant_mass");
        // The mission's final event is the authoritative transaction boundary.
        // Finalize here as well as from on_tick: an event may be delivered on
        // the update that ends the headless test, with no subsequent tick.
        this.done = true;
        finish(this);
    } else if evt.name == "MISSION_FAILED" {
        this.done = true;
        verdict(false, "production mission emitted MISSION_FAILED");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.main < 0 || this.rcs < 0 || this.lander < 0 || this.rover < 0 || this.goal < 0 {
        this.done = true;
        verdict(false, "production lander topology did not materialize");
        return;
    }

    let mass = get(this.main, "vehicle_mass_kg");
    let propellant = get(this.main, "propellant_mass");
    let thrust = get(this.main, "thrust_n");
    let inertia = get(this.main, "vehicle_inertia_yy_kg_m2");
    let rcs_flow = get(this.rcs, "mass_flow_kgs");
    let torque_x = get(this.lander, "torque_x");
    let torque_y = get(this.lander, "torque_y");
    let torque_z = get(this.lander, "torque_z");
    let velocity_x = get(this.lander, "velocity_x");
    let velocity_y = get(this.lander, "velocity_y");
    let velocity_z = get(this.lander, "velocity_z");
    let angular_x = get(this.lander, "angvel_x");
    let angular_y = get(this.lander, "angvel_y");
    let angular_z = get(this.lander, "angvel_z");
    if scalar_ok(mass) && this.initial_mass == () { this.initial_mass = mass; }
    if scalar_ok(propellant) && this.initial_propellant == () { this.initial_propellant = propellant; }
    if scalar_ok(thrust) && thrust > this.max_thrust { this.max_thrust = thrust; }
    if scalar_ok(rcs_flow) && rcs_flow > this.max_rcs_flow { this.max_rcs_flow = rcs_flow; }
    if scalar_ok(torque_x) && torque_x.abs() > this.max_pre_handoff_torque && !this.post_handoff {
        this.max_pre_handoff_torque = torque_x.abs();
    }
    if scalar_ok(torque_y) && torque_y.abs() > this.max_pre_handoff_torque && !this.post_handoff {
        this.max_pre_handoff_torque = torque_y.abs();
    }
    if scalar_ok(torque_z) && torque_z.abs() > this.max_pre_handoff_torque && !this.post_handoff {
        this.max_pre_handoff_torque = torque_z.abs();
    }
    if this.post_handoff {
        this.post_handoff_elapsed += dt();
        if this.post_handoff_elapsed >= 0.5 {
            if !this.stale_command_injected {
                this.stale_command_injected = true;
                this.stale_command_ok = set(this.lander, "guidance_throttle", 1.0)
                    && set(this.lander, "guidance_yaw", 1.0);
            }
            if scalar_ok(torque_x) && torque_x.abs() > this.max_post_handoff_torque {
                this.max_post_handoff_torque = torque_x.abs();
            }
            if scalar_ok(torque_y) && torque_y.abs() > this.max_post_handoff_torque {
                this.max_post_handoff_torque = torque_y.abs();
            }
            if scalar_ok(torque_z) && torque_z.abs() > this.max_post_handoff_torque {
                this.max_post_handoff_torque = torque_z.abs();
            }
            let throttle_cmd = get(this.lander, "throttle");
            if scalar_ok(throttle_cmd) && throttle_cmd.abs() > this.max_post_handoff_throttle {
                this.max_post_handoff_throttle = throttle_cmd.abs();
            }
            if scalar_ok(rcs_flow) && rcs_flow.abs() > this.max_post_handoff_rcs_flow {
                this.max_post_handoff_rcs_flow = rcs_flow.abs();
            }
            if scalar_ok(velocity_x) && scalar_ok(velocity_y) && scalar_ok(velocity_z) {
                let speed = sqrt(velocity_x * velocity_x + velocity_y * velocity_y +
                                 velocity_z * velocity_z);
                if this.post_handoff_initial_speed == () {
                    this.post_handoff_initial_speed = speed;
                }
                this.post_handoff_final_speed = speed;
            }
            if scalar_ok(angular_x) && scalar_ok(angular_y) && scalar_ok(angular_z) {
                let rate = sqrt(angular_x * angular_x + angular_y * angular_y +
                                angular_z * angular_z);
                if this.post_handoff_initial_rate == () {
                    this.post_handoff_initial_rate = rate;
                }
                this.post_handoff_final_rate = rate;
            }
        }
        // Allow the authored turbopump spool-down to complete before judging
        // physical main-engine thrust. The command boundary is checked during
        // the full post-handoff window; the engine's 0.08 s USD-authored spool
        // is not a controller pulse and must not be mistaken for one.
        if this.post_handoff_elapsed >= 2.0 {
            if scalar_ok(thrust) && thrust.abs() > this.max_post_handoff_thrust {
                this.max_post_handoff_thrust = thrust.abs();
            }
            this.post_handoff_quiet_elapsed += dt();
        }
        this.post_handoff_landing_state = get(this.lander, "landing_handoff");
        this.post_handoff_cutoff_state = get(this.lander, "landing_engine_cutoff");
    }
    if scalar_ok(inertia) == false || scalar_ok(mass) == false ||
       scalar_ok(propellant) == false || scalar_ok(rcs_flow) == false {
        if this.elapsed > 8.0 {
            this.done = true;
            verdict(false, "Modelica mass/propellant/inertia/RCS outputs were not finite");
            return;
        }
    }

    if high(this.leg_px, "contact") && high(this.leg_nx, "contact") &&
       high(this.leg_pz, "contact") && high(this.leg_nz, "contact") {
        this.native_contacts_seen = true;
    }
    if high(this.lander, "all_legs_contact") {
        this.model_contacts_seen = true;
    }

    if this.course_complete {
        this.done = true;
        finish(this);
        return;
    }
    if this.elapsed > 480.0 {
        this.done = true;
        let route_detail = "rover=" + world_pos(this.rover) +
            " target1=" + world_pos(find("/LanderTest/RoverTarget1")) +
            " target2=" + world_pos(find("/LanderTest/RoverTarget2")) +
            " target3=" + world_pos(find("/LanderTest/RoverTarget3"));
        verdict(false, "production descent/deployment/course did not finish in 480 simulated seconds; " + route_detail);
    }
}

fn finish(me) {
    let mass_ok = scalar_ok(me.initial_mass) && scalar_ok(me.final_mass);
    let prop_ok = scalar_ok(me.initial_propellant) && scalar_ok(me.final_propellant);
    let state_ok = mass_ok && prop_ok && me.initial_mass > 0.0 &&
                   me.final_mass > 0.0 && me.final_mass < me.initial_mass &&
                   me.final_propellant >= 0.0 && me.final_propellant < me.initial_propellant;
    let thrust_ok = me.max_thrust > 0.0;
    let rover_ok = me.course_complete && me.deployed;
    let pose_ok = t_bounded(world_pos(me.lander), 1000.0, "lander pose");
    let goal_ok = false;
    if me.rover >= 0 && me.goal >= 0 { goal_ok = distance(me.rover, me.goal) < 6.0; }
    let cutoff_latched = high(me.gnc, "landing_engine_cutoff_latched");
    let handoff_latched = high(me.gnc, "landing_handoff_latched");
    let transition_reason = "engine_cutoff=" + me.engine_cutoff +
                " flight_handoff=" + me.flight_handoff +
                " touchdown=" + me.touchdown + " deployed=" + me.deployed +
                " cutoff_latched=" + cutoff_latched +
                " handoff_latched=" + handoff_latched;
    let contact_reason = "event_sources_ok=" + me.event_sources_ok +
                " native_contacts=" + me.native_contacts_seen +
                " model_contacts=" + me.model_contacts_seen +
                " course=" + me.course_complete;
    let output_reason = "max_thrust=" + me.max_thrust +
                " max_rcs_flow=" + me.max_rcs_flow +
                " max_pre_handoff_torque=" + me.max_pre_handoff_torque;
    let post_reason = "post_handoff_elapsed=" + me.post_handoff_elapsed +
                " post_handoff_quiet_elapsed=" + me.post_handoff_quiet_elapsed +
                " max_post_handoff_torque=" + me.max_post_handoff_torque +
                " max_post_handoff_throttle=" + me.max_post_handoff_throttle +
                " max_post_handoff_thrust=" + me.max_post_handoff_thrust +
                " max_post_handoff_rcs_flow=" + me.max_post_handoff_rcs_flow +
                " initial_speed=" + me.post_handoff_initial_speed +
                " final_speed=" + me.post_handoff_final_speed +
                " initial_rate=" + me.post_handoff_initial_rate +
                " final_rate=" + me.post_handoff_final_rate;
    let state_reason = "landing_handoff=" + me.post_handoff_landing_state +
                " landing_engine_cutoff=" + me.post_handoff_cutoff_state +
                " stale_command_ok=" + me.stale_command_ok;
    let reason = transition_reason + " " + contact_reason + " " +
                output_reason + " " + post_reason + " " + state_reason;
    print("DESCENT_LANDER_RUNTIME: " + reason);
    if !me.engine_cutoff || !me.flight_handoff || !me.touchdown ||
       !cutoff_latched || !handoff_latched || !me.event_sources_ok ||
       !me.native_contacts_seen || !me.model_contacts_seen || !state_ok ||
       !thrust_ok || !rover_ok || !goal_ok || pose_ok != "" ||
       me.max_pre_handoff_torque <= 0.1 ||
       me.post_handoff_quiet_elapsed < 1.0 ||
       me.post_handoff_landing_state < 0.5 ||
       me.post_handoff_cutoff_state < 0.5 ||
       me.max_post_handoff_torque > 0.1 ||
       me.max_post_handoff_throttle > 0.001 ||
       me.max_post_handoff_thrust > 0.1 ||
       me.max_post_handoff_rcs_flow > 0.000001 ||
       !me.stale_command_injected || !me.stale_command_ok {
        verdict(false, reason + " state/motion contract failed");
        return;
    }
    verdict(true, "");
}

fn on_stop(me) {
    if !this.done {
        this.done = true;
        finish(this);
    }
}
