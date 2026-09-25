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

fn scenario_command_succeeded(result) {
    let succeeded = false;
    try { succeeded = result.ok == true; } catch (error) { }
    succeeded
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
    let first = find("/SandboxScene/Box");
    let second = find("/SandboxScene/Box50");
    let marker_entity = find("/SandboxScene/Box/Altimeter");
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
    let imu = find("/SandboxScene/Box/IMU");
    let altimeter = find("/SandboxScene/Box/Altimeter/Model");
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
    this.box = find("/SandboxScene/Box");
    this.imu = find("/SandboxScene/Box/IMU");
    this.altimeter = find("/SandboxScene/Box/Altimeter/Model");
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
    this.phase = "settle";
    print("[sensors] ── raw Avian + Modelica ─────────────────────────");
}

fn on_event(me, evt, ctx) {
    if evt.name == "scenario_actor_order_observation" {
        this.actor_order_result_received = true;
        this.actor_order_passed = evt.value == true;
    }
}

fn on_tick(me, ctx) {
    if this.phase == "done" { return; }

    if execution_context().generation != this.scene_generation {
        this.generation_stable = false;
    }

    this.telemetry_probe_count = telemetry_history_count(this.telemetry_probe_name);

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
            telemetry_probe_setup_ok: this.telemetry_probe_setup_ok,
            telemetry_probe_count: this.telemetry_probe_count,
        }, this.dependency_port_read_rejected, this.dependency_mutation_rejected,
            this.first_tick);
    }
}

fn verdict(s, dependency_port_read_rejected, dependency_mutation_rejected, first_tick) {
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
    f.push(t_true(s.telemetry_probe_setup_ok == true && s.telemetry_probe_count.to_float() >= 2.0,
        "the production telemetry query did not observe fixed-clock samples from the authored channel"));
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
