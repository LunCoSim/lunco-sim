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
       query_reads: ["CosimStatus"], required_inputs: [] }
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

fn emit_modelica_state_blocks(tick, identity, variables) {
    let fields = variables.keys();
    fields.sort();
    let block = 0;
    while block * 32 < fields.len() {
        let start = block * 32;
        let end = fields.len();
        if start + 32 < end { end = start + 32; }
        let row = "";
        for index in start..end {
            if row != "" { row += ";"; }
            let field = fields[index];
            row += field + "=" + state_scalar(variables[field]);
        }
        print("D4_MODEL_TRACE_V1|" + tick.to_string() + "|" + identity
            + "|" + fields.len().to_string() + "|" + block.to_string() + "|" + row);
        block += 1;
    }
}

fn modelica_clock_state(rover_paths, rovers, capture_values) {
    let clocks = [];
    let model_count = 0;
    let variables_count = 0;
    let complete = true;
    for i in 0..rovers.len() {
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
                clocks.push(rover_paths[i] + "/" + name(entity)
                    + "=" + state_scalar(current_time));
                let variables = ();
                try { variables = get(entity, "ModelicaModel.variables"); }
                catch (error) { }
                if variables != () && type_of(variables) == "map" {
                    let fields_count = variables.len();
                    variables_count += fields_count;
                    if fields_count == 0 { complete = false; }
                    if capture_values {
                        emit_modelica_state_blocks(sim_tick(),
                            rover_paths[i] + "/" + name(entity), variables);
                    }
                } else {
                    complete = false;
                    clocks.push(rover_paths[i] + "/" + name(entity) + "=variables-missing");
                }
            }
            for child in children(entity) { stack.push(child); }
        }
    }
    let count = clocks.len();
    clocks.sort();
    let row = "";
    for clock in clocks {
        if row != "" { row += ";"; }
        row += clock;
    }
    #{ complete: complete, count: model_count,
       variables_count: variables_count, row: row }
}

fn deterministic_state_sample(rover_paths, rovers, capture_modelica_values) {
    if rover_paths.len() != rovers.len() {
        return #{ valid: false, row: sim_tick().to_string() + "|invalid-roster" };
    }
    let valid = true;
    let row = sim_tick().to_string();
    for i in 0..rovers.len() {
        let path = rover_paths[i];
        let rover = rovers[i];
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
            row += ";" + path + "|invalid-state";
            continue;
        }
        let state_is_valid = state.usd_prim_path == path
            && state.body_mode == "dynamic"
            && state.physics_state_ready == true
            && state.physics_state_pending == false
            && state.linear_velocity_mps != ()
            && state.angular_velocity_radps != ();
        if !state_is_valid {
            valid = false;
            row += ";" + path + "|invalid-state";
            continue;
        }
        row += ";" + path;
        row += "|p=" + state_vector(position);
        row += "|q=" + state_vector(rotation);
        row += "|localTf=" + state_scalar(local_x) + "," + state_scalar(local_y)
            + "," + state_scalar(local_z);
        if capture_modelica_values {
            let authored_translation = ();
            try {
                let authored_prim = query("QueryUsdPrim", #{
                    path: state.usd_prim_path,
                    attrs: ["xformOp:translate"],
                    topology: true,
                });
                authored_translation = authored_prim.attrs["xformOp:translate"];
                let binding = authored_prim.topology.binding;
                let bevy_entity = binding.entity;
                if bevy_entity == () { valid = false; }
                row += "|bevyEntity=" + state_scalar(bevy_entity);
            } catch (error) { }
            row += "|authoredTf=" + state_vector(authored_translation);
        }
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
    }
    let modelica = modelica_clock_state(rover_paths, rovers, capture_modelica_values);
    row += "|modelica=" + modelica.row;
    #{ valid: valid && modelica.complete
            && modelica.count == rovers.len() && modelica.variables_count > 0,
       modelica_count: modelica.count, row: row }
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
    print("D4_ARTICULATED_BODY_TRACE_V2|" + tick.to_string()
        + "|" + rover_path + "|" + row);
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
    this.early_determinism_trace_captured = false;
    this.deterministic_trace_valid = true;
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

    let initial = deterministic_state_sample(this.rover_paths, this.rovers,
        this.rovers.len() == 20);
    this.deterministic_trace_valid = this.deterministic_trace_valid && initial.valid;
    this.deterministic_trace_ticks.push(sim_tick());
    print("D4_STATE_TRACE_V1|" + initial.row);
    print("[multi-rover-stress] discovered " + this.rovers.len() + " rover(s)");
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }
    this.t += dt();

    if !this.early_determinism_trace_captured {
        this.early_determinism_trace_captured = true;
        let capture_modelica_values = this.rovers.len() == 20
            && (sim_tick() < 10 || sim_tick() > 20);
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            capture_modelica_values);
        print("D4_EARLY_STATE_TRACE_V1|" + sample.row);
    }
    if this.rovers.len() == 20
        && (sim_tick() <= 30 || (sim_tick() <= 150 && sim_tick() % 10 == 0)) {
        let capture_modelica_values = sim_tick() >= 10 && sim_tick() <= 20;
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            capture_modelica_values);
        print("D4_TICK_STATE_TRACE_V1|" + sample.row);
    }
    if this.rovers.len() == 20 && (sim_tick() == 11 || sim_tick() == 80) {
        for i in 0..this.rovers.len() {
            trace_articulated_body_state(
                sim_tick(), this.rover_paths[i], this.rovers[i]);
        }
    }

    if this.phase == "settle" {
        if this.t < SETTLE_SECONDS {
            for rover in this.rovers { brake(rover); }
            return;
        }
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            this.rovers.len() == 20);
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.deterministic_trace_ticks.push(sim_tick());
        print("D4_STATE_TRACE_V1|" + sample.row);
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

    if this.t >= this.next_determinism_sample_s {
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            false);
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.deterministic_trace_ticks.push(sim_tick());
        print("D4_STATE_TRACE_V1|" + sample.row);
        this.next_determinism_sample_s += 3.0;
    }

    if this.t < SETTLE_SECONDS + DRIVE_SECONDS { return; }
    if this.deterministic_trace_ticks.len() == 0
        || this.deterministic_trace_ticks[this.deterministic_trace_ticks.len() - 1] != sim_tick() {
        let sample = deterministic_state_sample(this.rover_paths, this.rovers,
            false);
        this.deterministic_trace_valid = this.deterministic_trace_valid && sample.valid;
        this.deterministic_trace_ticks.push(sim_tick());
        print("D4_STATE_TRACE_V1|" + sample.row);
    }
    let checks = [];
    let modelica_profile = modelica_worker_step_profile();
    checks.push(t_true(this.authored_pose_valid,
        "authored rover transforms are active and seeded before physics runs"));
    checks.push(t_true(this.guidance_checked && this.guidance_command_ready,
        "every rover received its authored Modelica guidance setpoint"));
    checks.push(t_true(this.physics_profile_known && this.physics_compute_threads >= 1,
        "physics compute profile is available for deterministic-state comparison"));
    checks.push(t_true(this.deterministic_trace_valid
            && this.deterministic_trace_ticks.len() == 6,
        "one initial and five complete, path-ordered physics snapshots were captured"));
    checks.push(t_true(modelica_profile.participants.to_float() == 20.0
            && modelica_profile.entity_count.to_float()
                >= modelica_profile.participants.to_float()
            && modelica_profile.samples > 0.0
            && modelica_profile.mean_worker_step_us > 0.0,
        "CosimStatus did not expose diagnostics for all 20 Modelica participants"));
    checks.push(t_true(modelica_profile.compact_values_omitted == true,
        "compact CosimStatus copied full Modelica input/output maps"));
    let backlog = "unavailable";
    if modelica_profile.max_worker_backlog != () {
        backlog = modelica_profile.max_worker_backlog.to_string();
    }
    print("MODELica_STEP_PROFILE_V1|participants="
        + modelica_profile.participants.to_string()
        + "|entity_count=" + modelica_profile.entity_count.to_string()
        + "|samples=" + modelica_profile.samples.to_string()
        + "|mean_worker_step_us=" + modelica_profile.mean_worker_step_us.to_string()
        + "|max_worker_step_us=" + modelica_profile.max_worker_step_us.to_string()
        + "|max_response_latency_ms=" + modelica_profile.max_response_latency_ms.to_string()
        + "|max_worker_backlog=" + backlog);
    print("D4_PROFILE_V1|" + this.physics_compute_threads.to_string());
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
