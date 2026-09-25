// ── Raw Avian facts + Modelica conversions publish real numbers ────────────
//
// TEST-ONLY. Runs against scenes/tests/sensor.usda: a 100 kg box at rest with
// a reusable IMU conversion, a raw-ray/altimeter conversion, and native Avian
// collider contact. The first behavior call captures its declared Modelica
// ports and physics ports. It also verifies that scenario start already sees
// the committed scene generation and keeps it through its first behavior run.
// The initial altimeter miss must remain explicitly invalid until the first
// Avian ray sample arrives.
//
// The test reads Modelica outputs from the mounted conversion prims and
// contact directly from the collider. It also verifies stable identity and
// async scenario actor order through the production Rhai lifecycle path.

const EXPECT_RANGE = 0.8;    // box centre 0.5 + altimeter mount translation 0.3
const EXPECT_FORCE = 162.0;  // 100 kg * 1.62 m/s^2 lunar gravity

fn first_tick_read(entity, port) {
    let sample = #{ ok: false, value: (), error: "" };
    try {
        sample.value = get(entity, port);
        sample.ok = sample.value != ();
    } catch (error) {
        sample.error = error.to_string();
    }
    sample
}

fn first_tick_number(sample) {
    sample != () && sample.ok == true && sample.value != ()
        && sample.value.is_finite()
}

fn first_tick_trace_value(sample) {
    if sample != () && sample.ok == true && sample.value != () {
        sample.value.to_string()
    } else {
        "invalid"
    }
}

fn modelica_worker_step_diagnostics() {
    let status = query("CosimStatus", #{ include_values: false });
    if status == () || status.entities == () { return (); }
    let full_status = query("CosimStatus");
    for entity in status.entities {
        let diagnostics = entity.modelica_step_diagnostics;
        if diagnostics != () && diagnostics.samples.to_float() > 0.0 {
            let default_values_present = false;
            if full_status != () && full_status.values_included == true {
                for full_entity in full_status.entities {
                    if full_entity.name == entity.name {
                        default_values_present = full_entity.inputs.len()
                            + full_entity.outputs.len() > 0;
                    }
                }
            }
            return #{ diagnostics: diagnostics,
                compact_values_omitted: status.values_included == false
                    && entity.inputs.len() == 0 && entity.outputs.len() == 0,
                compact_status_retained: entity.status != ()
                    && entity.modelica_var_count.to_float() > 0.0
                    && entity.modelica_current_time != (),
                default_values_present: default_values_present };
        }
    }
    ()
}

fn scenario_command_succeeded(result) {
    let succeeded = false;
    try { succeeded = result.ok == true; } catch (error) { }
    succeeded
}

fn invalid_modelica_dependency_source() {
    let source = "fn simulation_dependencies(me, ctx) {\n";
    source += "  #{ modelica_entities: [ctx.declared_entity], required_inputs: [] }\n";
    source += "}\n";
    source += "fn on_start(me, ctx) { emit(\"sensor_invalid_dependency_started\", true); }\n";
    source
}

fn start_invalid_modelica_dependency_probe() {
    let target = find_path("/SandboxScene/Avatar");
    let unrelated = find_path("/SandboxScene/Box");
    if target < 0 || unrelated < 0 {
        return #{ accepted: false, target: target };
    }
    let result = cmd("RunScenario", #{
        target: target,
        source: invalid_modelica_dependency_source(),
        params: #{ declared_entity: unrelated },
    });
    #{ accepted: scenario_command_succeeded(result), target: target }
}

fn poll_invalid_modelica_dependency(target) {
    let rejected = false;
    if target < 0 { return rejected; }
    let status = query("ScriptStatus", #{ target: target });
    if status == () || status.state != "error" || status.diagnostics == () {
        return rejected;
    }
    for diagnostic in status.diagnostics {
        if diagnostic.message.contains("not a live Modelica participant") {
            rejected = true;
        }
    }
    rejected
}

fn undeclared_modelica_access_source() {
    let source = "fn simulation_dependencies(me, ctx) {\n";
    source += "  #{ modelica_entities: [], required_inputs: [] }\n";
    source += "}\n";
    source += "fn on_start(me, ctx) {\n";
    source += "  let result = cmd(\"SetPorts\", #{ target: ctx.modelica_target, writes: [[\"raw_acceleration_y\", 3.0]], seq: 0, tick: 0 });\n";
    source += "  let error = \"\";\n";
    source += "  try { error = result.error; } catch (failure) { }\n";
    source += "  emit(\"sensor_undeclared_command_rejected\", result != () && result.ok == false && error.contains(\"without this scenario's declared dependency\"));\n";
    source += "  let ports = query(\"ReadPorts\", #{ api_id: ctx.modelica_target });\n";
    source += "  emit(\"sensor_undeclared_readports_rejected\", ports != () && ports.ok == false && ports.error.contains(\"without this scenario's declared dependency\"));\n";
    source += "  port(ctx.modelica_target, \"specific_force_x\");\n";
    source += "}\n";
    source
}

fn start_undeclared_modelica_access_probe(target, modelica_target) {
    if target < 0 || modelica_target < 0 {
        return #{ accepted: false, target: target };
    }
    let result = cmd("RunScenario", #{
        target: target,
        source: undeclared_modelica_access_source(),
        params: #{ modelica_target: modelica_target },
    });
    #{ accepted: scenario_command_succeeded(result), target: target }
}

fn poll_undeclared_modelica_access(target) {
    if target < 0 { return false; }
    let status = query("ScriptStatus", #{ target: target });
    if status == () || status.state != "error" || status.diagnostics == () {
        return false;
    }
    for diagnostic in status.diagnostics {
        if diagnostic.message.contains("without this scenario's declared dependency") {
            return true;
        }
    }
    false
}

fn modelica_event_producer_source() {
    let source = "fn simulation_dependencies(me, ctx) {\n";
    source += "  #{ modelica_entities: [ctx.modelica_source], required_inputs: [] }\n";
    source += "}\n";
    source += "fn on_tick(me, ctx) {\n";
    source += "  if this.sent == true { return; }\n";
    source += "  this.sent = true;\n";
    source += "  emit(\"sensor_modelica_event_dependency_probe\", true);\n";
    source += "}\n";
    source
}

fn undeclared_modelica_event_consumer_source() {
    let source = "fn simulation_dependencies(me, ctx) {\n";
    source += "  #{ modelica_entities: [], required_inputs: [] }\n";
    source += "}\n";
    source += "fn on_event(me, evt, ctx) {\n";
    source += "  if evt.name == \"sensor_modelica_event_dependency_probe\" { emit(\"sensor_modelica_event_consumer_ran\", true); }\n";
    source += "}\n";
    source
}

fn start_undeclared_modelica_event_probe(modelica_source, receiver) {
    if modelica_source < 0 || receiver < 0 {
        return #{ producer_accepted: false, consumer_accepted: false, receiver: receiver };
    }
    let producer = cmd("RunScenario", #{
        target: modelica_source,
        source: modelica_event_producer_source(),
        params: #{ modelica_source: modelica_source },
    });
    let consumer = cmd("RunScenario", #{
        target: receiver,
        source: undeclared_modelica_event_consumer_source(),
        params: #{}
    });
    #{ producer_accepted: scenario_command_succeeded(producer),
       consumer_accepted: scenario_command_succeeded(consumer),
       receiver: receiver }
}

fn poll_undeclared_modelica_event(receiver) {
    if receiver < 0 { return false; }
    let status = query("ScriptStatus", #{ target: receiver });
    if status == () || status.state != "error" || status.diagnostics == () {
        return false;
    }
    for diagnostic in status.diagnostics {
        if diagnostic.message.contains("scenario received event")
            && diagnostic.message.contains("without this scenario's declared dependency") {
            return true;
        }
    }
    false
}

fn telemetry_history_count(name) {
    let catalog = query("ListTelemetryChannels", #{});
    if catalog == () || catalog.channels == () { return -1; }
    for channel in catalog.channels {
        if channel.name == name {
            let history = query("QueryTelemetryHistory", #{ key: channel.key, limit: 64 });
            let count = -1;
            try { count = history.count; } catch (error) { }
            return count;
        }
    }
    0
}

fn scenario_actor_order_source() {
    let source = "fn on_start(me, ctx) {\n";
    source += "  if ctx.role == \"writer\" {\n";
    source += "    set(ctx.marker_entity, \"Transform.translation.x\", ctx.marker);\n";
    source += "  } else {\n";
    source += "    let actual = get(ctx.marker_entity, \"Transform.translation.x\");\n";
    source += "    let observed = actual != () && (actual - ctx.marker).abs() < 0.01;\n";
    source += "    let restored = set(ctx.marker_entity, \"Transform.translation.x\", ctx.original_x);\n";
    source += "    let passed = observed && restored;\n";
    source += "    let outcome = \"FAIL\";\n";
    source += "    if passed { outcome = \"PASS\"; }\n";
    source += "    print(\"SCENARIO_ACTOR_ORDER_TRACE|\" + outcome);\n";
    source += "    emit(\"scenario_actor_order_observation\", passed);\n";
    source += "  }\n";
    source += "}\n";
    source
}

fn start_scenario_actor_order_probe(me) {
    let first = find_path("/SandboxScene/Box");
    let second = find_path("/SandboxScene/Box50");
    let marker_entity = find_path("/SandboxScene/Box/Altimeter");
    let original_x = ();
    try { original_x = get(marker_entity, "Transform.translation.x"); } catch (error) { }
    if me <= 0 || first < 0 || second < 0 || first == second
        || marker_entity < 0 || original_x == () {
        let details = "scenario actor ordering probe could not resolve stable hosts and scene state: me=";
        details += me.to_string() + " box=" + first.to_string();
        details += " box50=" + second.to_string() + " marker=" + marker_entity.to_string();
        details += " marker_x=" + first_tick_trace_value(#{ ok: original_x != (), value: original_x });
        print("SCENARIO_ACTOR_ORDER_SETUP_FAIL|" + details);
        return false;
    }

    let writer = first;
    let reader = second;
    if writer > reader { writer = second; reader = first; }
    let marker = original_x + 17.0;
    let source = scenario_actor_order_source();
    // Attach in descending identity order. Hook order must follow identity,
    // independent of command and compilation submission order.
    let read_actor = cmd("RunScenario", #{
        target: reader,
        source: source,
        params: #{
            role: "reader",
            marker_entity: marker_entity,
            marker: marker,
            original_x: original_x,
        },
    });
    let write_actor = cmd("RunScenario", #{
        target: writer,
        source: source,
        params: #{
            role: "writer",
            marker_entity: marker_entity,
            marker: marker,
            original_x: original_x,
        },
    });
    let accepted = scenario_command_succeeded(write_actor)
        && scenario_command_succeeded(read_actor);
    if !accepted {
        print("SCENARIO_ACTOR_ORDER_SETUP_FAIL|RunScenario rejected one of the actor probes");
    }
    accepted
}

fn simulation_dependencies(me, ctx) {
    let imu = find_path("/SandboxScene/Box/IMU");
    let altimeter = find_path("/SandboxScene/Box/Altimeter/Model");
    let port_read_rejected = false;
    let mutation_rejected = false;
    try {
        get(imu, "specific_force_x");
    } catch (error) {
        port_read_rejected = true;
    }
    try {
        set_physics_substeps(1);
    } catch (error) {
        mutation_rejected = true;
    }
    this.dependency_port_read_rejected = port_read_rejected;
    this.dependency_mutation_rejected = mutation_rejected;
    #{ modelica_entities: [imu, altimeter], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.t = 0.0;
    this.settle_s = 4.0;
    if this.start_count == () { this.start_count = 0; }
    this.start_count += 1;
    this.scene_generation = execution_context().generation;
    this.generation_stable = true;
    this.box = find_path("/SandboxScene/Box");
    this.imu = find_path("/SandboxScene/Box/IMU");
    this.altimeter = find_path("/SandboxScene/Box/Altimeter/Model");
    this.telemetry_probe_name = "sensor_scheduler_probe";
    let telemetry_probe = cmd("ControlTelemetry", #{
        entity: this.box,
        channel: this.telemetry_probe_name,
        port: "contact_force",
        rate_hz: 60.0,
        retention: 64,
        enabled: true,
    });
    this.telemetry_probe_setup_ok = scenario_command_succeeded(telemetry_probe);
    this.telemetry_probe_count = 0;
    this.first_tick = ();
    this.actor_identity = me;
    this.actor_order_setup_ok = start_scenario_actor_order_probe(me);
    this.actor_order_result_received = false;
    this.actor_order_passed = false;
    let invalid_dependency_probe = start_invalid_modelica_dependency_probe();
    this.invalid_dependency_setup_ok = invalid_dependency_probe.accepted;
    this.invalid_dependency_target = invalid_dependency_probe.target;
    this.invalid_dependency_plan_rejected = false;
    this.invalid_dependency_started = false;
    let undeclared_access_probe = start_undeclared_modelica_access_probe(
        find_path("/SandboxScene/Box/Altimeter"), this.imu,
    );
    this.undeclared_access_setup_ok = undeclared_access_probe.accepted;
    this.undeclared_access_target = undeclared_access_probe.target;
    this.undeclared_access_rejected = false;
    let undeclared_command_probe = undeclared_access_probe;
    this.undeclared_command_setup_ok = undeclared_command_probe.accepted;
    this.undeclared_command_result_received = false;
    this.undeclared_command_rejected = false;
    this.undeclared_readports_result_received = false;
    this.undeclared_readports_rejected = false;
    let event_receiver = find_path("/SandboxScene/Box/IMU");
    let undeclared_event_probe = start_undeclared_modelica_event_probe(
        this.altimeter, event_receiver,
    );
    this.undeclared_event_setup_ok = undeclared_event_probe.producer_accepted
        && undeclared_event_probe.consumer_accepted;
    this.undeclared_event_receiver = undeclared_event_probe.receiver;
    this.undeclared_event_rejected = false;
    this.undeclared_event_delivered = false;
    this.phase = "settle";
    print("[sensors] ── raw Avian + Modelica ─────────────────────────");
}

fn on_event(me, evt, ctx) {
    if evt.name == "scenario_actor_order_observation" {
        this.actor_order_result_received = true;
        this.actor_order_passed = evt.value == true;
    }
    if evt.name == "sensor_invalid_dependency_started" {
        this.invalid_dependency_started = evt.value == true;
    }
    if evt.name == "sensor_undeclared_command_rejected" {
        this.undeclared_command_result_received = true;
        this.undeclared_command_rejected = evt.value == true;
    }
    if evt.name == "sensor_undeclared_readports_rejected" {
        this.undeclared_readports_result_received = true;
        this.undeclared_readports_rejected = evt.value == true;
    }
    if evt.name == "sensor_modelica_event_consumer_ran" {
        this.undeclared_event_delivered = true;
    }
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }

    if execution_context().generation != this.scene_generation {
        this.generation_stable = false;
    }

    this.telemetry_probe_count = telemetry_history_count(this.telemetry_probe_name);
    if !this.invalid_dependency_plan_rejected {
        this.invalid_dependency_plan_rejected = poll_invalid_modelica_dependency(
            this.invalid_dependency_target,
        );
    }
    if !this.undeclared_access_rejected {
        this.undeclared_access_rejected = poll_undeclared_modelica_access(
            this.undeclared_access_target,
        );
    }
    if !this.undeclared_event_rejected {
        this.undeclared_event_rejected = poll_undeclared_modelica_event(
            this.undeclared_event_receiver,
        );
    }

    if this.box == () || this.box < 0 || this.imu == () || this.imu < 0
        || this.altimeter == () || this.altimeter < 0 {
        fail_fast("sensor conversion prim not found — wrong scene?", "SENSORS", "SENSORS");
        this.phase = "done";
        return;
    }

    if this.first_tick == () {
        let execution = execution_context();
        let physics_clock = clock_snapshot();
        this.first_tick = #{
            tick: sim_tick(),
            cycle: execution.cycle,
            phase: execution.phase,
            clock: execution.clock,
            generation: execution.generation,
            physics_compute_threads: physics_clock.physics_compute_threads,
            actor_identity: this.actor_identity,
            fx: first_tick_read(this.imu, "specific_force_x"),
            fy: first_tick_read(this.imu, "specific_force_y"),
            fz: first_tick_read(this.imu, "specific_force_z"),
            range: first_tick_read(this.altimeter, "range_m"),
            range_valid: first_tick_read(this.altimeter, "range_valid"),
            contact: first_tick_read(this.box, "contact"),
            force: first_tick_read(this.box, "contact_force"),
        };
        let trace = "SENSOR_FIRST_TICK_TRACE_V2|";
        trace += this.first_tick.tick.to_string() + "|";
        trace += this.first_tick.generation.to_string() + "|";
        trace += this.first_tick.physics_compute_threads.to_string() + "|";
        trace += this.first_tick.actor_identity.to_string() + "|";
        trace += first_tick_trace_value(this.first_tick.fx) + "|";
        trace += first_tick_trace_value(this.first_tick.fy) + "|";
        trace += first_tick_trace_value(this.first_tick.fz) + "|";
        trace += first_tick_trace_value(this.first_tick.range) + "|";
        trace += first_tick_trace_value(this.first_tick.range_valid) + "|";
        trace += first_tick_trace_value(this.first_tick.contact) + "|";
        trace += first_tick_trace_value(this.first_tick.force);
        print(trace);
    }

    this.t += dt();
    if this.t >= this.settle_s {
        this.phase = "done";
        let physics = physics_acceptance::sample(this.box);
        let contact_report = physics_acceptance::contact_report(this.box);
        let contact_acceptance = physics_acceptance::contact_acceptance(
            contact_report, 1, 1, 0.25,
        );
        let systems = physics_acceptance::system_evidence();
        let system_acceptance = physics_acceptance::system_acceptance(systems);
        let step_report = modelica_worker_step_diagnostics();
        let step_diagnostics = if step_report == () { () } else { step_report.diagnostics };
        let timing_profile = query("SimulationTimingProfile");
        let invalid_timing_options = query("SimulationTimingProfile", #{ unexpected: true });
        verdict(#{
            fx: get(this.imu, "specific_force_x"),
            fy: get(this.imu, "specific_force_y"),
            fz: get(this.imu, "specific_force_z"),
            range: get(this.altimeter, "range_m"),
            range_valid: get(this.altimeter, "range_valid"),
            contact: get(this.box, "contact"),
            force: get(this.box, "contact_force"),
            physics: physics,
            contact_acceptance: contact_acceptance,
            system_acceptance: system_acceptance,
            scene_generation: this.scene_generation,
            start_count: this.start_count,
            generation_stable: this.generation_stable,
            actor_identity: this.actor_identity,
            actor_order_setup_ok: this.actor_order_setup_ok,
            actor_order_result_received: this.actor_order_result_received,
            actor_order_passed: this.actor_order_passed,
            invalid_dependency_setup_ok: this.invalid_dependency_setup_ok,
            invalid_dependency_plan_rejected: this.invalid_dependency_plan_rejected,
            invalid_dependency_started: this.invalid_dependency_started,
            undeclared_access_setup_ok: this.undeclared_access_setup_ok,
            undeclared_access_rejected: this.undeclared_access_rejected,
            undeclared_command_setup_ok: this.undeclared_command_setup_ok,
            undeclared_command_result_received: this.undeclared_command_result_received,
            undeclared_command_rejected: this.undeclared_command_rejected,
            undeclared_readports_result_received: this.undeclared_readports_result_received,
            undeclared_readports_rejected: this.undeclared_readports_rejected,
            undeclared_event_setup_ok: this.undeclared_event_setup_ok,
            undeclared_event_rejected: this.undeclared_event_rejected,
            undeclared_event_delivered: this.undeclared_event_delivered,
            telemetry_probe_setup_ok: this.telemetry_probe_setup_ok,
            telemetry_probe_count: this.telemetry_probe_count,
            modelica_step_diagnostics: step_diagnostics,
            cosim_status_values_omitted: if step_report == () { false } else { step_report.compact_values_omitted },
            cosim_status_default_values_present: if step_report == () { false } else { step_report.default_values_present },
            cosim_status_compact_fields_retained: if step_report == () { false } else { step_report.compact_status_retained },
            simulation_timing_profile: timing_profile,
            simulation_timing_profile_invalid_options_rejected: invalid_timing_options != ()
                && invalid_timing_options.ok == false,
        }, this.dependency_port_read_rejected, this.dependency_mutation_rejected,
            this.first_tick, step_diagnostics);
    }
}

fn verdict(s, dependency_port_read_rejected, dependency_mutation_rejected, first_tick,
           modelica_step_diagnostics) {
    let f = [];
    f.push(t_true(dependency_port_read_rejected == true,
        "dependency planning allowed a live Modelica port read"));
    f.push(t_true(dependency_mutation_rejected == true,
        "dependency planning allowed a direct world mutation"));
    f.push(t_present(s.fx, "IMU specific_force_x output"));
    f.push(t_present(s.fy, "IMU specific_force_y output"));
    f.push(t_present(s.fz, "IMU specific_force_z output"));
    f.push(t_present(s.range, "altimeter range_m output"));
    f.push(t_present(s.range_valid, "altimeter range_valid output"));
    f.push(t_present(s.contact, "native Avian contact port"));
    f.push(t_present(s.force, "native Avian contact_force port"));
    f.push(t_true(s.physics.ok == true && s.physics.contact != (),
        "generic physics evidence did not include the live contact snapshot"));
    f.push(t_true(s.contact_acceptance.ok == true,
        "generic contact acceptance did not observe a resting manifold"));
    f.push(t_true(s.system_acceptance.ok == true,
        "generic readiness/binding diagnostics report an unresolved runtime fault"));
    f.push(t_true(s.scene_generation != () && s.start_count == 1,
        "scenario on_start ran once after a committed scene generation was available"));
    f.push(t_true(s.generation_stable == true,
        "the active scenario retained the same committed scene generation through its run"));
    f.push(t_true(s.actor_identity > 0,
        "the authored scenario host did not expose its stable GlobalEntityId"));
    f.push(t_true(s.actor_order_setup_ok == true,
        "the Rhai actor order acceptance programs were not attached"));
    f.push(t_true(s.actor_order_result_received == true && s.actor_order_passed == true,
        "stable GlobalEntityId actor order was not observed through the production event path"));
    f.push(t_true(s.invalid_dependency_setup_ok == true,
        "the malformed Modelica dependency probe did not attach"));
    f.push(t_true(s.invalid_dependency_plan_rejected == true,
        "simulation_dependencies accepted a live non-Modelica entity"));
    f.push(t_true(s.invalid_dependency_started == false,
        "scenario on_start ran after its Modelica dependency plan was rejected"));
    f.push(t_true(s.undeclared_access_setup_ok == true,
        "the undeclared Modelica access probe did not attach"));
    f.push(t_true(s.undeclared_access_rejected == true,
        "a scenario accessed a Modelica participant declared only by another scenario"));
    f.push(t_true(s.undeclared_command_setup_ok == true
        && s.undeclared_command_result_received == true
        && s.undeclared_command_rejected == true,
        "a targeted SetPorts command bypassed this scenario's Modelica dependency plan"));
    f.push(t_true(s.undeclared_readports_result_received == true
        && s.undeclared_readports_rejected == true,
        "ReadPorts exposed another scenario's Modelica participant without a declared dependency"));
    f.push(t_true(s.undeclared_event_setup_ok == true
        && s.undeclared_event_rejected == true
        && s.undeclared_event_delivered == false,
        "a scenario consumed an event from a Modelica participant it did not declare"));
    f.push(t_true(s.telemetry_probe_setup_ok == true && s.telemetry_probe_count.to_float() >= 2.0,
        "the production telemetry query did not observe fixed-clock samples from the authored channel"));
    f.push(t_present(modelica_step_diagnostics, "Modelica worker step diagnostics"));
    f.push(t_true(s.cosim_status_values_omitted == true,
        "compact CosimStatus did not omit full Modelica input/output maps"));
    f.push(t_true(s.cosim_status_default_values_present == true,
        "CosimStatus did not retain full input/output maps by default"));
    f.push(t_true(s.cosim_status_compact_fields_retained == true,
        "compact CosimStatus omitted its status, Modelica count, or current time"));
    f.push(t_present(s.simulation_timing_profile, "fixed simulation timing profile"));
    f.push(t_true(s.simulation_timing_profile_invalid_options_rejected == true,
        "SimulationTimingProfile accepted unknown query options"));
    if s.simulation_timing_profile != () {
        let tick = s.simulation_timing_profile.fixed_tick;
        let fixed_loop = s.simulation_timing_profile.fixed_loop;
        f.push(t_true(tick.window_samples.to_float() > 0.0
            && tick.window_samples.to_float() <= 240.0
            && tick.ticks_total.to_float() > 0.0
            && tick.service_ms_p50 != () && tick.service_ms_p95 != ()
            && tick.service_ms_p99 != () && tick.service_ms_max != ()
            && tick.service_budget_ms_p50 != ()
            && tick.service_budget_ms_p95 != ()
            && tick.service_budget_ms_p99 != ()
            && tick.service_budget_ms_max != ()
            && tick.service_budget_ms_p95.to_float() > 0.0
            && tick.service_ms_p50 <= tick.service_ms_p95
            && tick.service_ms_p95 <= tick.service_ms_p99
            && tick.service_ms_p99 <= tick.service_ms_max
            && tick.service_budget_ms_p50 <= tick.service_budget_ms_p95
            && tick.service_budget_ms_p95 <= tick.service_budget_ms_p99
            && tick.service_budget_ms_p99 <= tick.service_budget_ms_max
            && tick.service_budget_exceedances_total != (),
            "fixed-tick timing did not expose bounded service percentiles"));
        f.push(t_true(fixed_loop.window_samples.to_float() > 0.0
            && fixed_loop.window_samples.to_float() <= 240.0
            && fixed_loop.latest_steps != () && fixed_loop.latest_steps.to_float() > 0.0
            && fixed_loop.latest_service_ms != ()
            && fixed_loop.latest_max_delta_limited_simulation_secs != ()
            && fixed_loop.latest_max_delta_limited_simulation_secs.to_float() >= 0.0
            && fixed_loop.max_delta_limited_simulation_secs_total != ()
            && fixed_loop.max_delta_limited_simulation_secs_total.to_float() >= 0.0
            && fixed_loop.service_ms_p50 <= fixed_loop.service_ms_p95
            && fixed_loop.service_ms_p95 <= fixed_loop.service_ms_p99
            && fixed_loop.service_ms_p99 <= fixed_loop.service_ms_max
            && fixed_loop.latest_fractional_overstep_secs != (),
            "fixed-loop profile did not expose service, steps, and remaining overstep"));
        print("SENSOR_TIMING_PROFILE|tick_ms_p50=" + tick.service_ms_p50.to_string()
            + "|tick_ms_p95=" + tick.service_ms_p95.to_string()
            + "|tick_ms_p99=" + tick.service_ms_p99.to_string()
            + "|tick_ms_max=" + tick.service_ms_max.to_string()
            + "|tick_budget_ms_p95=" + tick.service_budget_ms_p95.to_string()
            + "|tick_service_budget_exceedances="
            + tick.service_budget_exceedances_total.to_string()
            + "|fixed_loop_ms_p95=" + fixed_loop.service_ms_p95.to_string()
            + "|fixed_loop_steps=" + fixed_loop.latest_steps.to_string()
            + "|max_delta_limited_sim_s="
            + fixed_loop.latest_max_delta_limited_simulation_secs.to_string());
    }
    if modelica_step_diagnostics != () {
        f.push(t_true(modelica_step_diagnostics.samples.to_float() > 0.0
            && modelica_step_diagnostics.worker_step_last_ns.to_float() > 0.0
            && modelica_step_diagnostics.worker_step_mean_ns.is_finite()
            && modelica_step_diagnostics.response_latency_last_ns.to_float() > 0.0,
            "Modelica diagnostics did not separate solver service from response latency"));
        f.push(t_true(modelica_step_diagnostics.worker_backlog_count_last != (),
            "native Modelica diagnostics did not expose worker backlog at step start"));
    }
    f.push(t_present(first_tick, "first authoritative behavior tick snapshot"));
    if first_tick != () {
        f.push(t_true(first_tick.tick >= 0 && first_tick.cycle == "simulation"
            && first_tick.phase == "behavior" && first_tick.clock == "simulation"
            && first_tick.generation == s.scene_generation,
            "first dependency reads ran in the scene generation captured by on_start"));
        f.push(t_true(first_tick.physics_compute_threads >= 1,
            "first dependency reads observed the admitted physics compute profile"));
        f.push(t_true(first_tick.actor_identity == s.actor_identity,
            "scenario identity changed between start and its first behavior tick"));
        f.push(t_true(first_tick_number(first_tick.fx)
            && first_tick_number(first_tick.fy)
            && first_tick_number(first_tick.fz),
            "first behavior tick read finite IMU outputs from its declared Modelica dependency"));
        f.push(t_true(first_tick_number(first_tick.range)
            && first_tick_number(first_tick.range_valid),
            "first behavior tick read finite altimeter outputs from its declared Modelica dependency"));
        f.push(t_true(first_tick_number(first_tick.contact)
            && first_tick_number(first_tick.force),
            "first behavior tick read finite physics outputs from its admitted body"));
        if first_tick_number(first_tick.range_valid) {
            f.push(t_true(first_tick.range_valid.value >= 0.0
                && first_tick.range_valid.value <= 1.0,
                "first behavior tick observed out-of-range altimeter validity"));
        }
        if first_tick_number(first_tick.range)
            && first_tick_number(first_tick.range_valid)
            && first_tick.range_valid.value >= 1.0 {
            f.push(t_rel(first_tick.range.value, EXPECT_RANGE, 20.0,
                "first behavior tick did not observe the authored altimeter ray input"));
        } else if first_tick_number(first_tick.range)
            && first_tick_number(first_tick.range_valid)
            && first_tick.range_valid.value == 0.0 {
            f.push(t_true(first_tick.range.value == 0.0,
                "invalid initial altimeter input must not appear as a measured range"));
        }
    }

    if s.fy != () {
        f.push(t_true(s.fx.is_finite() && s.fy.is_finite() && s.fz.is_finite(),
            "the Modelica IMU conversion returned a non-finite reading"));
    }
    if s.range != () {
        f.push(t_rel(s.range, EXPECT_RANGE, 20.0,
            "the raw Avian ray does not see the ground at the authored mount"));
    }
    if s.range_valid != () {
        f.push(t_true(s.range_valid > 0.5,
            "the altimeter conversion did not preserve raw ray validity"));
    }
    if s.contact != () {
        f.push(t_true(s.contact > 0.5,
            "native Avian contact is not active for the resting box"));
    }
    if s.force != () {
        f.push(t_rel(s.force, EXPECT_FORCE, 25.0,
            "native Avian contact force is not the box's lunar weight"));
    }

    print("[sensors] ── results ─────────────────────────────────────────");
    print("[sensors]   force   " + r2(s.fx) + ", " + r2(s.fy) + ", " + r2(s.fz));
    print("[sensors]   range   " + r4(s.range) + " m (expect " + EXPECT_RANGE + ")");
    print("[sensors]   contact " + r2(s.contact) + "  force " + r2(s.force) +
          " N (expect " + EXPECT_FORCE + ")");

    report_verdict(f, "SENSORS", "SENSORS");
}
