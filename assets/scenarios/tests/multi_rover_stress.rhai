// Production stress-matrix correctness check for the authored 4/8/20 rover
// scenes. The scene owns the roster and lane positions; this script discovers
// that roster, publishes one guidance setpoint per vehicle, and measures each
// vehicle while its authored Modelica controller owns continuous drive output.

const SETTLE_SECONDS = 3.0;
const DRIVE_SECONDS = 10.0;
const DRIVE_DISTANCE_M = 20.0;
const DRIVE_THROTTLE = 0.20;

fn simulation_dependencies(me, ctx) {
    let group = find_path("/MultiRoverStress/Rovers");
    let modelica_entities = [];
    if group != () && group >= 0 {
        for rover in children(group) {
            modelica_entities.push(rover);
        }
    }
    #{ modelica_entities: modelica_entities, entity_reads: [], entity_writes: [],
       query_reads: ["CosimStatus", "SceneTestParameters", "ReadDeterminismReference"],
       required_inputs: [] }
}

fn state_scalar(value) {
    if value == () { return "<unit>"; }
    value.to_string()
}

fn state_vector(value) {
    if value == () { return "<unit>"; }
    let text = "";
    for component in value {
        if text != "" { text += ","; }
        text += state_scalar(component);
    }
    text
}

fn modelica_worker_step_profile() {
    let status = query("CosimStatus", #{
        include_values: false,
        include_entities: false,
    });
    if status == () || status.ok == false || status.modelica_step_profile == () {
        return #{ participants: 0, samples: 0.0, mean_worker_step_us: 0.0,
            max_worker_step_us: 0.0, max_response_latency_ms: 0.0,
            max_worker_backlog: (), compact_values_omitted: false,
            entity_count: 0 };
    }
    let profile = status.modelica_step_profile;
    #{ participants: profile.participants,
        samples: profile.samples.to_float(),
        mean_worker_step_us: profile.worker_step_mean_ns / 1000.0,
        max_worker_step_us: profile.worker_step_max_ns.to_float() / 1000.0,
        max_response_latency_ms: profile.response_latency_max_ns.to_float() / 1_000_000.0,
        max_worker_backlog: profile.worker_backlog_count_max,
        compact_values_omitted: status.values_included == false
            && status.entities_included == false && status.entities.len() == 0,
        entity_count: status.entity_count }
}

fn close_scalar(actual, expected, tolerance) {
    actual != () && (actual - expected).abs() <= tolerance
}

fn vector_close(actual, expected, tolerance) {
    if actual == () || actual.len() != expected.len() { return false; }
    for i in 0..expected.len() {
        if !close_scalar(actual[i], expected[i], tolerance) { return false; }
    }
    true
}

fn read_authored_translation(path) {
    try {
        let prim = query("QueryUsdPrim", #{
            path: path,
            attrs: ["xformOp:translate"],
        });
        if prim != () && prim.ok != false && prim.attrs != () {
            return prim.attrs["xformOp:translate"];
        }
    } catch (error) { }
    ()
}

fn modelica_clock_state(rovers, capture_values, profile, tick, kind) {
    let model_count = 0;
    let variables_count = 0;
    let complete = true;
    let failure = "";
    for i in 0..rovers.len() {
        let lane_x = -2.5 * (rovers.len() - 1).to_float()
            + 5.0 * i.to_float();
        let stack = [rovers[i]];
        let visits = 0;
        while stack.len() > 0 && visits < 512 {
            visits += 1;
            let entity = stack.pop();
            let current_time = ();
            try { current_time = get(entity, "ModelicaModel.current_time"); }
            catch (error) { }
            if current_time != () {
                model_count += 1;
                let variables = ();
                try { variables = get(entity, "ModelicaModel.variables"); }
                catch (error) { }
                if variables != () && type_of(variables) == "map" {
                    let fields_count = variables.len();
                    variables_count += fields_count;
                    if fields_count == 0 { complete = false; }
                    if capture_values && profile != "" {
                        let current = modelica_reference_failure(profile, kind,
                            tick, state_scalar(lane_x), variables);
                        if failure == "" && current != "" { failure = current; }
                    }
                } else {
                    complete = false;
                }
            }
            for child in children(entity) { stack.push(child); }
        }
    }
    #{
        complete: complete,
        count: model_count,
        variables_count: variables_count,
        failure: failure,
    }
}

fn deterministic_state_sample(rover_paths, rovers, capture_modelica_values,
                              profile, modelica_kind) {
    if rover_paths.len() != rovers.len() {
        return #{
            valid: false,
            physics_records: [],
            modelica_count: 0,
            modelica_failure: "",
        };
    }
    let clocks = clock_snapshot();
    if clocks == () || clocks.fixed_dt_s == () || clocks.fixed_delta_s == ()
        || clocks.fixed_elapsed_s == ()
        || clocks.physics_dt_s == () || clocks.physics_elapsed_s == ()
        || clocks.physics_paused == () || clocks.physics_holds_available != true
        || clocks.physics_hold_reasons == () {
        return #{
            valid: false,
            physics_records: [],
            modelica_count: 0,
            modelica_failure: "",
        };
    }
    let tick = sim_tick();
    let expected_fixed_elapsed = tick.to_float() * clocks.fixed_dt_s;
    let expected_fixed_delta = if tick > 0 { clocks.fixed_dt_s } else { 0.0 };
    let previous_physics_tick = if tick > 0 { (tick - 1).to_float() } else { 0.0 };
    let expected_physics_elapsed = previous_physics_tick * clocks.fixed_dt_s;
    let no_physics_holds = clocks.physics_hold_reasons.len() == 0;
    let clocks_synchronized = close_scalar(
        clocks.fixed_elapsed_s, expected_fixed_elapsed, 0.000000001)
        && close_scalar(clocks.fixed_delta_s, expected_fixed_delta, 0.000000001)
        && close_scalar(clocks.physics_elapsed_s, expected_physics_elapsed, 0.000000001)
        && clocks.physics_paused == false
        && no_physics_holds;
    let valid = clocks_synchronized;
    let hold_names = "";
    for reason in clocks.physics_hold_reasons {
        if hold_names != "" { hold_names += ","; }
        hold_names += state_scalar(reason);
    }
    if !clocks_synchronized {
        print("D4_CLOCK_SYNC_FAIL|tick=" + tick.to_string()
            + "|fixedDt=" + state_scalar(clocks.fixed_dt_s)
            + "|fixedDelta=" + state_scalar(clocks.fixed_delta_s)
            + "|fixedElapsed=" + state_scalar(clocks.fixed_elapsed_s)
            + "|physicsDelta=" + state_scalar(clocks.physics_dt_s)
            + "|physicsElapsed=" + state_scalar(clocks.physics_elapsed_s)
            + "|physicsPaused=" + state_scalar(clocks.physics_paused)
            + "|barrierHeld=" + state_scalar(clocks.barrier_held)
            + "|physicsHolds=" + hold_names);
    }

    let physics_records = [];
    for i in 0..rovers.len() {
        let path = rover_paths[i];
        let rover = rovers[i];
        let lane_x = -2.5 * (rovers.len() - 1).to_float()
            + 5.0 * i.to_float();
        let identity = "RoverAtX[" + state_scalar(lane_x) + "]";
        let position = world_pos(rover);
        let rotation = world_rotation(rover);
        let local_x = ();
        let local_y = ();
        let local_z = ();
        try { local_x = get(rover, "Transform.translation.x"); }
        catch (error) { }
        try { local_y = get(rover, "Transform.translation.y"); }
        catch (error) { }
        try { local_z = get(rover, "Transform.translation.z"); }
        catch (error) { }
        let state = query("QueryPhysicsState", #{ id: rover });
        let avian_contact = contact_debug(rover);
        if position == () || rotation == () || state == () || avian_contact == () {
            valid = false;
            continue;
        }
        let state_is_valid = state.usd_prim_path == path
            && state.body_mode == "dynamic"
            && state.physics_state_ready == true
            && state.physics_state_pending == false
            && state.physics_pose_seeded == true
            && state.physics_admission_requested == false
            && state.physics_initialization_pending == false
            && state.rigid_body_disabled == false
            && state.mass_kg != () && state.mass_kg > 0.0
            && state.computed_mass_finite == true
            && state.computed_inertia_finite == true
            && state.linear_velocity_mps != ()
            && state.angular_velocity_radps != ();
        if !state_is_valid {
            valid = false;
            continue;
        }

        if capture_modelica_values {
            let authored_translation = read_authored_translation(path);
            valid = valid && vector_close(
                authored_translation, [lane_x, 1.6, 0.0], 0.0001);
        }

        let row = identity;
        row += "|clock=" + state_scalar(clocks.fixed_dt_s)
            + "," + state_scalar(clocks.fixed_delta_s)
            + "," + state_scalar(clocks.fixed_elapsed_s)
            + "," + state_scalar(clocks.physics_elapsed_s)
            + "," + state_scalar(clocks.physics_paused)
            + "," + state_scalar(clocks.barrier_held);
        row += "|physicsHolds=" + hold_names;
        row += "|p=" + state_vector(position);
        row += "|q=" + state_vector(rotation);
        row += "|localTf=" + state_scalar(local_x) + "," + state_scalar(local_y)
            + "," + state_scalar(local_z);
        row += "|laneX=" + state_scalar(lane_x);
        row += "|v=" + state_vector(state.linear_velocity_mps);
        row += "|w=" + state_vector(state.angular_velocity_radps);
        row += "|sleep=" + state_scalar(state.sleeping);
        row += "|avianContact=" + state_scalar(avian_contact.pair_count)
            + "," + state_scalar(avian_contact.touching_pair_count)
            + "," + state_scalar(avian_contact.manifold_count)
            + "," + state_scalar(avian_contact.max_penetration)
            + "," + state_vector(avian_contact.normal_impulse_vector);
        row += "|piloted=" + state_scalar(port(rover, "piloted"));
        row += "|poseSeeded=" + state_scalar(state.physics_pose_seeded);
        row += "|poseAuthoritative=" + state_scalar(state.physics_pose_authoritative);
        row += "|admissionRequested=" + state_scalar(state.physics_admission_requested);
        row += "|initializationPending=" + state_scalar(state.physics_initialization_pending);
        row += "|initializationInvalid=" + state_scalar(state.physics_initialization_invalid);
        row += "|bodyDisabled=" + state_scalar(state.rigid_body_disabled);
        row += "|collider=" + state_scalar(state.collider_present);
        row += "|supportFootprint=" + state_scalar(state.support_footprint_count);
        row += "|mass=" + state_scalar(state.mass_kg);
        row += "|com=" + state_vector(state.center_of_mass_m);
        row += "|inertia=" + state_vector(state.inertia_principal_kgm2);
        row += "|support=" + state_scalar(state.support_contact_count);
        for wheel in state.wheel_contacts {
            if wheel.wheel_usd_prim_path == () { valid = false; }
            row += "~" + state_scalar(wheel.wheel_usd_prim_path);
            row += "," + state_scalar(wheel.contact_valid);
            row += "," + state_scalar(wheel.hit_usd_prim_path_available);
            row += "," + state_scalar(wheel.hit_usd_prim_path);
            row += "," + state_scalar(wheel.distance_available);
            row += "," + state_scalar(wheel.distance_m);
            row += "," + state_vector(wheel.normal);
            row += "," + state_scalar(wheel.normal_force_n);
            row += "," + state_scalar(wheel.suspension_compression_m);
            row += "," + state_vector(wheel.tire_force_n);
            row += "," + state_scalar(wheel.sample_tick);
        }
        physics_records.push(#{ identity: identity, row: row });
    }

    let modelica = modelica_clock_state(rovers, capture_modelica_values,
        profile, tick, modelica_kind);
    #{
        valid: valid && modelica.complete
            && modelica.count == rovers.len() && modelica.variables_count > 0,
        modelica_count: modelica.count,
        physics_records: physics_records,
        modelica_failure: modelica.failure,
    }
}

fn trace_articulated_body_state(tick, rover_path, rover) {
    let stack = [rover];
    let records = [];
    let visits = 0;
    while stack.len() > 0 && visits < 512 {
        visits += 1;
        let entity = stack.pop();
        let state = query("QueryPhysicsState", #{ id: entity });
        if state != () && state.usd_prim_path != ()
            && state.body_mode == "dynamic" {
            let position = world_pos(entity);
            let rotation = world_rotation(entity);
            let contact = contact_debug(entity);
            if position != () && rotation != () && contact != () {
                let record = state.usd_prim_path;
                record += "|p=" + state_vector(position);
                record += "|q=" + state_vector(rotation);
                record += "|v=" + state_vector(state.linear_velocity_mps);
                record += "|w=" + state_vector(state.angular_velocity_radps);
                record += "|authoredMass=" + state_scalar(state.authored_mass_kg);
                record += "|massOverride=" + state_scalar(state.mass_override_active);
                record += "|inertiaOverride=" + state_scalar(state.inertia_override_active);
                record += "|computedMassFinite=" + state_scalar(state.computed_mass_finite);
                record += "|computedInertiaFinite=" + state_scalar(state.computed_inertia_finite);
                record += "|mass=" + state_scalar(state.mass_kg);
                record += "|inertia=" + state_vector(state.inertia_principal_kgm2);
                record += "|contact=" + state_scalar(contact.pair_count);
                record += "," + state_scalar(contact.touching_pair_count);
                record += "," + state_scalar(contact.manifold_count);
                record += "," + state_scalar(contact.max_penetration);
                record += "," + state_vector(contact.normal_impulse_vector);
                records.push(record);
            }
        }
        for child in children(entity) { stack.push(child); }
    }
    records.sort();
    let row = "";
    for record in records {
        if row != "" { row += "~"; }
        row += record;
    }
    #{
        tick: tick,
        rover_path: rover_path,
        bodies: row,
        valid: records.len() > 0,
    }
}

fn state_row_difference(expected_row, actual_row) {
    let expected_fields = expected_row.split("|");
    let actual_fields = actual_row.split("|");
    let common_fields = if expected_fields.len() < actual_fields.len() {
        expected_fields.len()
    } else {
        actual_fields.len()
    };
    for i in 0..common_fields {
        if expected_fields[i] != actual_fields[i] {
            return "field expected [" + expected_fields[i]
                + "] actual [" + actual_fields[i] + "]";
        }
    }
    if expected_fields.len() > actual_fields.len() {
        return "field expected [" + expected_fields[common_fields]
            + "] actual [<missing>]";
    }
    if actual_fields.len() > expected_fields.len() {
        return "field expected [<missing>] actual ["
            + actual_fields[common_fields] + "]";
    }
    "rows differ without a differing field"
}

fn reference_value_failure(profile, kind, tick, identity, actual) {
    let expected = query("ReadDeterminismReference", #{
        profile: profile,
        kind: kind,
        tick: tick.to_string(),
        identity: identity,
    });
    if expected == () {
        return "reference query returned no value for " + kind
            + " tick " + tick.to_string() + " identity " + identity;
    }
    if expected.ok == false {
        return "reference query failed for " + kind
            + " tick " + tick.to_string() + ": " + expected.error;
    }
    if expected.found != true || expected.value == () {
        return "reference is missing " + kind
            + " tick " + tick.to_string() + " identity " + identity;
    }
    if actual != expected.value {
        return "exact state differs for " + kind
            + " tick " + tick.to_string() + " identity " + identity
            + " (numeric_tolerance=0); "
            + state_row_difference(expected.value, actual);
    }
    ""
}

fn modelica_reference_failure(profile, kind, tick, identity, variables) {
    let expected = query("ReadDeterminismReference", #{
        profile: profile,
        kind: kind,
        tick: tick.to_string(),
        identity: identity,
    });
    if expected == () {
        return "reference query returned no value for " + kind
            + " tick " + tick.to_string() + " identity " + identity;
    }
    if expected.ok == false {
        return "reference query failed for " + kind
            + " tick " + tick.to_string() + ": " + expected.error;
    }
    if expected.found != true || expected.value == () {
        return "reference is missing " + kind
            + " tick " + tick.to_string() + " identity " + identity;
    }
    let fields = variables.keys();
    fields.sort();
    let expected_fields = expected.value.split(";");
    if fields.len() != expected_fields.len() {
        return "Modelica variable count differs for tick " + tick.to_string()
            + " identity " + identity + ": expected "
            + expected_fields.len().to_string() + " actual "
            + fields.len().to_string();
    }
    for i in 0..fields.len() {
        let field = fields[i];
        let actual = field + "=" + state_scalar(variables[field]);
        if actual != expected_fields[i] {
            return "Modelica state differs at tick " + tick.to_string()
                + " identity " + identity + " variable " + field
                + " (numeric_tolerance=0); expected [" + expected_fields[i]
                + "] actual [" + actual + "]";
        }
    }
    ""
}

fn sample_reference_failure(profile, tick, sample, rover_count,
                            physics_kind, compare_modelica) {
    if sample.physics_records.len() != rover_count {
        return "physics roster changed at tick " + tick.to_string();
    }
    let failure = "";
    for record in sample.physics_records {
        let current = reference_value_failure(
            profile, physics_kind, tick, record.identity, record.row);
        if failure == "" && current != "" { failure = current; }
    }
    if compare_modelica {
        if sample.modelica_count != rover_count {
            return "Modelica roster changed at tick " + tick.to_string();
        }
        if sample.modelica_failure != "" { return sample.modelica_failure; }
    }
    failure
}

fn articulated_reference_failure(profile, tick, records, rover_paths, kind) {
    if records.len() != rover_paths.len() {
        return "articulated rover roster changed at tick " + tick.to_string();
    }
    let failure = "";
    for record in records {
        if record.valid != true {
            return "no articulated body state was available for " + record.rover_path;
        }
        let current = reference_value_failure(
            profile, kind, tick, record.rover_path, record.bodies);
        if failure == "" && current != "" { failure = current; }
    }
    failure
}

fn deterministic_profile_selection(rover_count) {
    let parameters = query("SceneTestParameters");
    if parameters == () || parameters.ok == false {
        return #{
            valid: false, profile: "",
            failure: "scene-test parameters are unavailable",
            final_tick: -1,
        };
    }
    let thread_setting = if parameters.threads.to_float() == 0.0 {
        "default"
    } else if parameters.threads.to_float() == 1.0 {
        "single"
    } else {
        "custom"
    };
    let profile = "";
    if parameters.jitter == 0.0 {
        let profile_thread = if thread_setting == "default" {
            "default"
        } else if thread_setting == "single" {
            "serial"
        } else {
            "custom"
        };
        profile = "scene-" + rover_count.to_string() + "-" + profile_thread;
    } else {
        profile = "jitter-" + parameters.jitter.to_string()
            + "-seed-" + parameters.seed.to_string();
    }
    let reference = query("ReadDeterminismReference", #{
        profile: profile, kind: "metadata",
    });
    if reference == () || reference.ok == false {
        let detail = if reference != () && reference.error != () {
            reference.error
        } else {
            "reference profile was not found"
        };
        return #{
            valid: false, profile: profile,
            failure: detail, final_tick: -1,
        };
    }
    let expected = reference.parameters;
    let scene = "assets/scenes/tests/multi_rover_stress_"
        + rover_count.to_string() + ".usda";
    let metadata_valid = expected.rover_count.to_float() == rover_count.to_float()
        && expected.jitter.to_float() == parameters.jitter.to_float()
        && expected.seed.to_string() == parameters.seed.to_string()
        && expected.tick_hz.to_float() == parameters.tick_hz.to_float()
        && expected.thread_setting == thread_setting
        && expected.scene == scene
        && reference.comparison.numeric_tolerance == 0.0
        && reference.comparison.final_stage_required == true
        && reference.comparison.selected_lifecycle_checkpoints.to_float() == 6.0
        && reference.comparison.selected_articulated_checkpoint_ticks == ["11", "80"]
        && reference.first_behavior_tick == "1"
        && reference.effective_compute_width.to_float() >= 1.0;
    #{
        valid: metadata_valid,
        profile: profile,
        failure: if metadata_valid { "" } else {
            "reference profile metadata does not match the requested run"
        },
        final_tick: reference.final_tick,
    }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.phase = "settle";
    this.rovers = [];
    this.rover_paths = [];
    this.previous = [];
    this.distance = [];
    this.targets = [];
    this.deterministic_trace_ticks = [];
    this.determinism_profile = "";
    this.determinism_comparison_valid = false;
    this.determinism_comparison_failure = "";
    this.determinism_mismatch_rejected = false;
    this.determinism_final_tick = "";
    this.final_stage_tick = ();
    this.early_determinism_trace_captured = false;
    this.last_observed_tick = sim_tick();
    this.first_missing_tick = ();
    this.on_tick_sequence_valid = true;
    this.deterministic_trace_valid = true;
    this.on_start_tick = sim_tick();
    this.first_behavior_tick_valid = false;
    this.guidance_checked = false;
    this.guidance_command_ready = false;
    this.next_determinism_sample_s = 6.0;
    let clock = clock_snapshot();
    this.physics_profile_known = clock != () && clock.physics_profile_known == true;
    this.physics_compute_threads = if this.physics_profile_known {
        clock.physics_compute_threads
    } else {
        0
    };

    let group = find_path("/MultiRoverStress/Rovers");
    if group == () || group < 0 {
        print("MULTI-ROVER STRESS: FAIL — /MultiRoverStress/Rovers is absent");
        emit("MULTI_ROVER_STRESS", "FAIL");
        this.phase = "done";
        return;
    }
    let rover_by_path = #{};
    let discovered_rovers = [];
    for rover in children(group) {
        if world_pos(rover) != () {
            discovered_rovers.push(rover);
            let state = query("QueryPhysicsState", #{ id: rover });
            if state != () && state.usd_prim_path != () {
                rover_by_path[state.usd_prim_path] = rover;
            } else {
                this.deterministic_trace_valid = false;
            }
        }
    }
    let rover_paths = rover_by_path.keys();
    rover_paths.sort();
    if rover_paths.len() == discovered_rovers.len() {
        for path in rover_paths {
            this.rover_paths.push(path);
            this.rovers.push(rover_by_path[path]);
        }
    } else {
        this.rovers = discovered_rovers;
        this.deterministic_trace_valid = false;
    }
    let ground = query("QueryUsdPrim", #{
        path: "/MultiRoverStress/Ground",
        attrs: ["xformOp:translate", "xformOp:scale"],
    });
    let ground_center_x = ();
    let ground_half_width = ();
    if ground != () && ground.ok != false && ground.attrs != () {
        let ground_translation = ground.attrs["xformOp:translate"];
        let ground_scale = ground.attrs["xformOp:scale"];
        if ground_translation != () && ground_scale != ()
            && ground_translation.len() == 3 && ground_scale.len() == 3 {
            ground_center_x = ground_translation[0];
            ground_half_width = ground_scale[0] / 2.0;
        }
    }
    let lane_half_span = 2.5 * (this.rovers.len() - 1).to_float();
    this.authored_pose_valid = this.rover_paths.len() == this.rovers.len()
        && ground_center_x != () && ground_half_width != ()
        && ground_center_x - ground_half_width <= -lane_half_span - 2.5
        && ground_center_x + ground_half_width >= lane_half_span + 2.5;
    if this.authored_pose_valid {
        for i in 0..this.rovers.len() {
            let rover = this.rovers[i];
            let authored = query("QueryUsdPrim", #{
                path: this.rover_paths[i],
                attrs: ["xformOp:translate", "xformOpOrder"],
            });
            let translation = if authored == () || authored.attrs == () {
                ()
            } else {
                authored.attrs["xformOp:translate"]
            };
            let order = if authored == () || authored.attrs == () {
                ()
            } else {
                authored.attrs.xformOpOrder
            };
            let local_x = ();
            let local_y = ();
            let local_z = ();
            try { local_x = get(rover, "Transform.translation.x"); }
            catch (error) { }
            try { local_y = get(rover, "Transform.translation.y"); }
            catch (error) { }
            try { local_z = get(rover, "Transform.translation.z"); }
            catch (error) { }

            let expected_x = -2.5 * (this.rovers.len() - 1).to_float()
                + 5.0 * i.to_float();
            this.authored_pose_valid = this.authored_pose_valid
                && authored != () && authored.ok != false
                && order != () && order.len() == 1
                && order[0] == "xformOp:translate"
                && vector_close(translation, [expected_x, 1.6, 0.0], 0.0001)
                && close_scalar(local_x, expected_x, 0.0001)
                && close_scalar(local_y, 1.6, 0.0001)
                && close_scalar(local_z, 0.0, 0.0001);
        }
    }
    for rover in this.rovers {
        this.previous.push(());
        this.distance.push(0.0);
    }

    // A one-rover run is not a stress matrix. The exact 4/8/20 cardinality is
    // authored by each USD fixture; discovery keeps this reader reusable.
    if this.rovers.len() < 4 {
        print("MULTI-ROVER STRESS: FAIL — discovered only " + this.rovers.len() +
              " rover(s); minimum matrix cardinality is four");
        emit("MULTI_ROVER_STRESS", "FAIL");
        this.phase = "done";
        return;
    }

    let profile_selection = deterministic_profile_selection(this.rovers.len());
    this.determinism_profile = profile_selection.profile;
    this.determinism_comparison_valid = profile_selection.valid;
    this.determinism_comparison_failure = profile_selection.failure;
    this.determinism_final_tick = profile_selection.final_tick;

    let initial = deterministic_state_sample(this.rover_paths, this.rovers,
        true, this.determinism_profile, "modelica");
    this.deterministic_trace_valid = this.deterministic_trace_valid && initial.valid;
    this.deterministic_trace_ticks.push(sim_tick());
    if profile_selection.valid {
        let failure = sample_reference_failure(this.determinism_profile,
            sim_tick(), initial, this.rovers.len(), "physics", true);
        this.determinism_comparison_valid = this.determinism_comparison_valid
            && failure == "";
        if this.determinism_comparison_failure == "" {
            this.determinism_comparison_failure = failure;
        }
        if initial.physics_records.len() > 0 {
            let record = initial.physics_records[0];
            let reference_row = query("ReadDeterminismReference", #{
                profile: this.determinism_profile,
                kind: "physics",
                tick: sim_tick().to_string(),
                identity: record.identity,
            });
            let mismatch = reference_value_failure(this.determinism_profile,
                "physics", sim_tick(), record.identity,
                if reference_row != () && reference_row.ok != false
                    && reference_row.found == true {
                    reference_row.value + "|deliberate-mismatch"
                } else {
                    "unavailable"
                });
            let expected_mismatch = "exact state differs for physics tick "
                + sim_tick().to_string() + " identity " + record.identity
                + " (numeric_tolerance=0); field expected [<missing>] "
                + "actual [deliberate-mismatch]";
            this.determinism_mismatch_rejected = mismatch == expected_mismatch;
        }
    }
    print("[multi-rover-stress] discovered " + this.rovers.len() + " rover(s)");
}

fn on_tick(me, ctx) {
    let current_tick = sim_tick();
    if current_tick != this.last_observed_tick + 1 {
        this.on_tick_sequence_valid = false;
        if this.first_missing_tick == () {
            this.first_missing_tick = this.last_observed_tick + 1;
            print("D4_ON_TICK_GAP|expected=" + this.first_missing_tick.to_string()
                + "|observed=" + current_tick.to_string());
        }
    }
    this.last_observed_tick = current_tick;
    if this.phase == "done" { return; }
    let scenario_tick = sim_tick();
    this.t += dt();

    if !this.early_determinism_trace_captured {
        this.early_determinism_trace_captured = true;
        this.first_behavior_tick_valid = scenario_tick == 1;
        let capture_modelica_values = scenario_tick < 10 || scenario_tick > 20;
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            capture_modelica_values, this.determinism_profile, "modelica");
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.first_behavior_tick_valid = scenario_tick == 1;
        this.deterministic_trace_ticks.push(scenario_tick);
        if this.determinism_profile != "" {
            let failure = sample_reference_failure(this.determinism_profile,
                scenario_tick, sample, this.rovers.len(), "physics", true);
            this.determinism_comparison_valid = this.determinism_comparison_valid
                && failure == "";
            if this.determinism_comparison_failure == "" {
                this.determinism_comparison_failure = failure;
            }
        }
    }
    if scenario_tick == 11 || scenario_tick == 80 {
        let articulated = [];
        for i in 0..this.rovers.len() {
            articulated.push(trace_articulated_body_state(
                scenario_tick, this.rover_paths[i], this.rovers[i]));
        }
        if this.determinism_profile != "" {
            let failure = articulated_reference_failure(this.determinism_profile,
                scenario_tick, articulated, this.rover_paths, "articulated");
            this.determinism_comparison_valid = this.determinism_comparison_valid
                && failure == "";
            if this.determinism_comparison_failure == "" {
                this.determinism_comparison_failure = failure;
            }
        }
    }

    if this.phase == "settle" {
        if this.t < SETTLE_SECONDS {
            for rover in this.rovers { brake(rover); }
            return;
        }
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            true, this.determinism_profile, "modelica");
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.deterministic_trace_ticks.push(scenario_tick);
        if this.determinism_profile != "" {
            let failure = sample_reference_failure(this.determinism_profile,
                scenario_tick, sample, this.rovers.len(), "physics", true);
            this.determinism_comparison_valid = this.determinism_comparison_valid
                && failure == "";
            if this.determinism_comparison_failure == "" {
                this.determinism_comparison_failure = failure;
            }
        }
        for i in 0..this.rovers.len() {
            let at = world_pos(this.rovers[i]);
            this.previous[i] = at;
            if at != () {
                this.targets.push([at[0], at[1], at[2] - DRIVE_DISTANCE_M]);
            } else {
                // Preserve the roster index if a subject disappears during
                // settling; later samples must not address a shorter route
                // array and hide the real failure behind a script exception.
                this.targets.push(());
            }
        }
        for i in 0..this.rovers.len() {
            modelica_waypoint(this.rovers[i], this.targets[i],
                DRIVE_THROTTLE, 0.75, 0.0);
        }
        this.phase = "drive";
        print("[multi-rover-stress] drive started for " + this.rovers.len() + " rover(s)");
        return;
    }

    if !this.guidance_checked {
        this.guidance_checked = true;
        this.guidance_command_ready = true;
        for i in 0..this.rovers.len() {
            let active = port(this.rovers[i], "program_active");
            let enabled = port(this.rovers[i], "autopilot_enable");
            let target_x = port(this.rovers[i], "autopilot_target_x");
            let target_z = port(this.rovers[i], "autopilot_target_z");
            this.guidance_command_ready = this.guidance_command_ready
                && active != () && active >= 0.5
                && enabled != () && enabled >= 0.5
                && close_scalar(target_x, this.targets[i][0], 0.0001)
                && close_scalar(target_z, this.targets[i][2], 0.0001);
        }
    }

    for i in 0..this.rovers.len() {
        let at = world_pos(this.rovers[i]);
        if at != () && this.previous[i] != () {
            this.distance[i] += seg(this.previous[i], at);
            this.previous[i] = at;
        }
    }

    if this.t >= this.next_determinism_sample_s
        && this.t < SETTLE_SECONDS + DRIVE_SECONDS {
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            false, this.determinism_profile, "modelica");
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.deterministic_trace_ticks.push(scenario_tick);
        if this.determinism_profile != "" {
            let failure = sample_reference_failure(this.determinism_profile,
                scenario_tick, sample, this.rovers.len(), "physics", false);
            this.determinism_comparison_valid = this.determinism_comparison_valid
                && failure == "";
            if this.determinism_comparison_failure == "" {
                this.determinism_comparison_failure = failure;
            }
        }
        this.next_determinism_sample_s += 3.0;
    }

    if this.t < SETTLE_SECONDS + DRIVE_SECONDS { return; }
    let final_sample = deterministic_state_sample(this.rover_paths, this.rovers,
        true, this.determinism_profile, "final_modelica");
    this.deterministic_trace_valid = this.deterministic_trace_valid && final_sample.valid;
    if this.determinism_profile != "" {
        let failure = sample_reference_failure(this.determinism_profile,
            scenario_tick, final_sample, this.rovers.len(),
            "final_physics", true);
        this.determinism_comparison_valid = this.determinism_comparison_valid
            && failure == "";
        if this.determinism_comparison_failure == "" {
            this.determinism_comparison_failure = failure;
        }
    }
    this.final_stage_tick = scenario_tick;
    let final_articulated = [];
    for i in 0..this.rovers.len() {
        final_articulated.push(trace_articulated_body_state(
            scenario_tick, this.rover_paths[i], this.rovers[i]));
    }
    if this.determinism_profile != "" {
        let failure = articulated_reference_failure(this.determinism_profile,
            scenario_tick, final_articulated, this.rover_paths, "final_articulated");
        this.determinism_comparison_valid = this.determinism_comparison_valid
            && failure == "";
        if this.determinism_comparison_failure == "" {
            this.determinism_comparison_failure = failure;
        }
    }
    let checks = [];
    checks.push(t_true(this.on_start_tick == 0,
        "on_start ran at the shared global SimTick zero baseline"));
    let modelica_profile = modelica_worker_step_profile();
    checks.push(t_true(this.authored_pose_valid,
        "authored rover transforms are active and seeded before physics runs"));
    checks.push(t_true(this.first_behavior_tick_valid,
        "the first on_tick ran one admitted tick after scenario startup"));
    checks.push(t_true(this.on_tick_sequence_valid,
        "on_tick observed every admitted SimTick exactly once; first gap="
            + state_scalar(this.first_missing_tick)));
    checks.push(t_true(this.guidance_checked && this.guidance_command_ready,
        "every rover received its authored Modelica guidance setpoint"));
    checks.push(t_true(this.physics_profile_known && this.physics_compute_threads >= 1,
        "physics compute profile is available for deterministic-state comparison"));
    checks.push(t_true(this.deterministic_trace_valid
            && this.deterministic_trace_ticks == [0, 1, 180, 360, 540, 720],
        "six selected startup and lifecycle physics checkpoints were captured"));
    checks.push(t_true(this.final_stage_tick == sim_tick()
            && this.final_stage_tick.to_string() == this.determinism_final_tick,
        "the explicit final simulation stage was captured at the ending SimTick"));
    checks.push(t_true(this.determinism_comparison_valid,
        "physics, Modelica, and articulated states exactly match the selected reference; "
            + this.determinism_comparison_failure));
    checks.push(t_true(this.determinism_mismatch_rejected,
        "the Rhai comparison rejects an intentionally altered physics row"));
    checks.push(t_true(modelica_profile.participants.to_float()
                == this.rovers.len().to_float()
            && modelica_profile.entity_count.to_float()
                >= modelica_profile.participants.to_float()
            && modelica_profile.samples > 0.0
            && modelica_profile.mean_worker_step_us > 0.0,
        "CosimStatus did not expose diagnostics for every rover Modelica participant"));
    checks.push(t_true(modelica_profile.compact_values_omitted == true,
        "compact CosimStatus copied full Modelica input/output maps"));
    for i in 0..this.rovers.len() {
        let at = world_pos(this.rovers[i]);
        checks.push(t_true(at != (), "rover " + i + " was removed during the stress run"));
        if at != () {
            checks.push(t_bounded(at, 500.0, "rover " + i + " position"));
        }
        checks.push(t_true(this.distance[i] > 0.25,
            "rover " + i + " did not move under the shared patrol command"));
    }
    for rover in this.rovers { modelica_waypoint_stop(rover); }
    report_verdict(checks, "MULTI-ROVER STRESS", "MULTI_ROVER_STRESS");
    this.phase = "done";
}

fn on_stop(me, ctx) {
    for rover in this.rovers {
        if rover != () && rover >= 0 { modelica_waypoint_stop(rover); }
    }
}
