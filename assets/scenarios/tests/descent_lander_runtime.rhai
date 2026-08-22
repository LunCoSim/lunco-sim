//! Production descent/lander runtime acceptance.
//!
//! This observes the authored `lander_ops.usda` mission in one process.  It
//! proves the real Modelica propulsion projection is live (thrust, propellant,
//! mass and inertia), the authored touchdown/deployment events occur, and the
//! production rover completes its route through the normal autopilot/control
//! boundary.  No test-only physics or direct state writes are used.

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
    this.pad_px = find("/LanderTest/Lander/LegPX/PadPX");
    this.pad_nx = find("/LanderTest/Lander/LegNX/PadNX");
    this.pad_pz = find("/LanderTest/Lander/LegPZ/PadPZ");
    this.pad_nz = find("/LanderTest/Lander/LegNZ/PadNZ");
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
    } else if evt.name == "lander_touchdown" {
        if evt.source != this.lander { this.event_sources_ok = false; }
        this.touchdown = true;
    } else if evt.name == "rover_deployed" {
        this.deployed = true;
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
    if scalar_ok(mass) && this.initial_mass == () { this.initial_mass = mass; }
    if scalar_ok(propellant) && this.initial_propellant == () { this.initial_propellant = propellant; }
    if scalar_ok(thrust) && thrust > this.max_thrust { this.max_thrust = thrust; }
    if scalar_ok(rcs_flow) && rcs_flow > this.max_rcs_flow { this.max_rcs_flow = rcs_flow; }
    if scalar_ok(inertia) == false || scalar_ok(mass) == false ||
       scalar_ok(propellant) == false || scalar_ok(rcs_flow) == false {
        if this.elapsed > 8.0 {
            this.done = true;
            verdict(false, "Modelica mass/propellant/inertia/RCS outputs were not finite");
            return;
        }
    }

    if high(this.pad_px, "contact") && high(this.pad_nx, "contact") &&
       high(this.pad_pz, "contact") && high(this.pad_nz, "contact") {
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
        verdict(false, "production descent/deployment/course did not finish in 480 simulated seconds");
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
    let reason = "engine_cutoff=" + me.engine_cutoff +
                " flight_handoff=" + me.flight_handoff +
                " touchdown=" + me.touchdown + " deployed=" + me.deployed +
                " cutoff_latched=" + cutoff_latched +
                " handoff_latched=" + handoff_latched +
                " event_sources_ok=" + me.event_sources_ok +
                " native_contacts=" + me.native_contacts_seen +
                " model_contacts=" + me.model_contacts_seen +
                " course=" + me.course_complete + " max_thrust=" + me.max_thrust +
                " max_rcs_flow=" + me.max_rcs_flow;
    if !me.engine_cutoff || !me.flight_handoff || !me.touchdown ||
       !cutoff_latched || !handoff_latched || !me.event_sources_ok ||
       !me.native_contacts_seen || !me.model_contacts_seen || !state_ok ||
       !thrust_ok || !rover_ok || !goal_ok || pose_ok != "" {
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
