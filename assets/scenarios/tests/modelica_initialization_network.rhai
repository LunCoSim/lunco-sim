const ROOT = "/ModelicaRuntimeContracts";
const SOURCE = "lunco://models/tests/InitialFeedback.mo";
const PAUSE_PROBE = "lunco.test.modelica_network.pause_probe";
fn slow_period() { 1.0 / 30.0 }

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CosimStatus", "ReadModelicaStepSamples",
                    "ListTelemetryChannels", "QueryTelemetryHistory"], required_inputs: []}
}
fn attach(doc, name, gain, bias) {
    assembly_edit::attach_program(doc, #{edit_target: "@runtime@", host_path: ROOT,
        name: name, source_asset: SOURCE,
        inputs: [#{name: "feedback", type_name: "double", default_value: 0.0, connection: ()},
                 #{name: "clockFeedback", type_name: "double", default_value: 0.0, connection: ()},
                 #{name: "gain", type_name: "double", default_value: gain, connection: ()},
                 #{name: "bias", type_name: "double", default_value: bias, connection: ()}],
        outputs: [#{name: "sample", type_name: "double", connections: []},
                  #{name: "solverTime", type_name: "double", connections: []},
                  #{name: "clockSeen", type_name: "double", connections: []}],
        realtime_safe: false})
}
fn connect(doc, target, source) {
    #{SetConnection: #{edit_target: "@runtime@", path: ROOT + "/" + target,
        name: "inputs:feedback", type_name: "double", sources: [ROOT + "/" + source + ".outputs:sample"]}}
}
fn row(rows, name) {
    for item in rows { if item.name == ROOT + "/" + name { return item; } }
    ()
}
fn finish(state, failures) {
    state.done = true;
    report_verdict(failures, "MODELICA INITIALIZATION NETWORK", "MODELICA_INITIALIZATION_NETWORK");
    state
}
fn start_case(negative) {
    let state = #{done: false, ticks: 0, failures: [], doc: (), negative: negative, phase: 0};
    let title = if negative { "modelica_initialization_failure" } else { "modelica_initialization_network" };
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.title == title
            && doc.runtime_context.kind == "local_twin" { state.doc = doc.doc_id; }
    }
    if state.doc == () { return finish(state, ["runtime contract document is missing"]); }
    if state.negative { expect_runtime_fault("modelica-initialization-failed"); }
    let gain = if state.negative { 1.0 } else { 0.5 };
    for spec in [["FeedbackA", 1.0, 0.0], ["FeedbackB", gain, 1.0]] {
        let admitted = attach(state.doc, spec[0], spec[1], spec[2]);
        if admitted.ok != true { return finish(state, [admitted.to_string()]); }
    }
    let ops = [connect(state.doc, "FeedbackA", "FeedbackB"), connect(state.doc, "FeedbackB", "FeedbackA")];
    if !state.negative {
        ops.push(#{SetConnection: #{edit_target: "@runtime@", path: ROOT + "/FeedbackB",
            name: "inputs:clockFeedback", type_name: "double", sources: [ROOT + "/FeedbackA.outputs:solverTime"]}});
        ops.push(#{SetAttribute: #{edit_target: "@runtime@", path: ROOT + "/FeedbackB",
            name: "lunco:program:communicationPeriod", type_name: "double", value: slow_period().to_string()}});
    }
    let result = cmd("ApplyUsdOps", #{doc_id: state.doc, ops: ops});
    if result.ok != true { return finish(state, [result.to_string()]); }
    state
}
fn step_case(state) {
    if state.done { return state; }
    state.ticks += 1;
    if state.ticks > 3000 { return finish(state, ["coupled runtime admission timed out"]); }
    let status = query("CosimStatus");
    let a = row(status.entities, "FeedbackA"); let b = row(status.entities, "FeedbackB");
    if a == () || b == () { return state; }
    if state.negative {
        if a.status == "Running" || b.status == "Running" {
            state = finish(state, ["inconsistent initial equations were admitted"]);
        }
        return state;
    }
    if a.status.starts_with("Error:") || b.status.starts_with("Error:") {
        return finish(state, [a.to_string(), b.to_string()]);
    }
    if a.status != "Running" || b.status != "Running" { return state; }
    if state.phase != 0 { return step_clock_case(state, a, b); }
    if a.modelica_current_time < 2.0 * slow_period() || b.modelica_current_time < 2.0 * slow_period() {
        return state;
    }
    state.failures += [
        t_true(status.last_coupled_initialization != ()
            && status.last_coupled_initialization.construction_profile.session_ns.to_float() > 0.0,
            "the certified one-shot initializer retains its worker-issued execution receipt"),
        t_true(abs(a.outputs.sample - 2.0) < 1.0e-9, "first participant admits the coupled initial state"),
        t_true(abs(b.outputs.sample - 2.0) < 1.0e-9, "second participant admits the same initial coordinate"),
        t_true(a.pending_initial_connections == () && b.pending_initial_connections == (), "all causal edges are real"),
        t_true(abs(a.modelica_communication_period_secs - dt()) < 1.0e-9,
            "the default participant exchanges on each physics tick"),
        t_true(abs(b.modelica_communication_period_secs - slow_period()) < 1.0e-9,
            "the authored participant retains its explicit 30 Hz exchange"),
        t_true(abs(a.modelica_current_time - b.modelica_current_time) <= slow_period() + 1.0e-9,
            "mixed-rate coupled participants remain within their declared exchange intervals")
    ];
    subscribe(PAUSE_PROBE);
    state.a_id = find_path(ROOT + "/FeedbackA");
    state.b_id = find_path(ROOT + "/FeedbackB");
    track_entity_read(state.a_id);
    track_entity_read(state.b_id);
    state.phase = 1;
    state.clock_start = clock_snapshot();
    state.a_start = a.modelica_target_time;
    state.b_start = b.modelica_target_time;
    set_rate(4.0);
    state
}

fn check_clock(state, a, b, label) {
    let clock = clock_snapshot();
    let elapsed = clock.fixed_elapsed_s - state.clock_start.fixed_elapsed_s;
    let master_elapsed = (clock.sim_tick - state.clock_start.sim_tick).to_float()
        * a.modelica_communication_period_secs;
    let samples = query("ReadModelicaStepSamples", #{targets: [
        #{api_id: state.a_id, input_names: ["feedback"], output_names: ["solverTime"]},
        #{api_id: state.b_id, input_names: ["feedback", "clockFeedback"], output_names: ["solverTime", "clockSeen"]}
    ]}).entities;
    state.failures += [
        t_true(abs(execution_context().time_seconds - elapsed_seconds()) < 1.0e-13
            && dt() == clock.simulation_dt_s,
            label + ": simulation scripts and timers inherit the integer master clock"),
        t_true(abs(a.modelica_target_time - state.a_start - master_elapsed) < 1.0e-13,
            label + ": default Modelica time is derived from the authoritative physics tick"),
        t_true(abs(b.modelica_target_time - state.b_start - master_elapsed) < 1.0e-13,
            label + ": subrate Modelica time is derived from the same authoritative physics tick"),
        t_true(clock.virtual_rate == (if state.phase == 1 { 4.0 } else if state.phase == 2 { 0.25 } else { 1.0 }),
            label + ": the shared transport applies the requested rate"),
        t_true(abs(clock.fixed_delta_s - state.clock_start.fixed_delta_s) < 1.0e-12,
            label + ": rate does not change the physics quantum"),
        t_true(abs(clock.physics_elapsed_s - state.clock_start.physics_elapsed_s - elapsed) < 1.0e-8,
            label + ": Avian and co-simulation use the same elapsed physics interval"),
        t_true(abs(a.modelica_target_time - state.a_start - elapsed) < 1.0e-8
            && a.modelica_target_time - a.modelica_current_time <= dt() + 1.0e-8,
            label + ": default participant advances exactly with fixed physics time"),
        t_true(abs(b.modelica_target_time - state.b_start - elapsed) < 1.0e-8
            && b.modelica_target_time - b.modelica_current_time <= slow_period() + 1.0e-8,
            label + ": slower participant stays on its declared communication grid"),
        t_true(samples.len == 2 && samples[0].sample != () && samples[1].sample != ()
            && abs(samples[0].sample.sampled_outputs.solverTime - samples[0].sample.output_time_s) < 1.0e-8
            && abs(samples[1].sample.sampled_outputs.solverTime - samples[1].sample.output_time_s) < 1.0e-8,
            label + ": Modelica time equals the committed solver coordinate"),
        t_true(samples[1].sample != ()
            && abs(samples[1].sample.sampled_inputs.clockFeedback - samples[1].sample.input_time_s) < 1.0e-8
            && samples[1].sample.sampled_outputs.clockSeen == samples[1].sample.sampled_inputs.clockFeedback,
            label + ": the slower participant integrates inputs sampled at its interval start")
    ];
    state
}

fn step_clock_case(state, a, b) {
    let clock = clock_snapshot();
    if state.phase == 5 {
        // Match the pre-dispatch phase used by the other running samples.
        // Paused events observe the completed preceding transaction instead.
        state.phase = 6;
        state.clock_start = clock;
        state.a_start = a.modelica_target_time;
        state.b_start = b.modelica_target_time;
        return state;
    }
    if clock.sim_tick - state.clock_start.sim_tick < 12 { return state; }
    state = check_clock(state, a, b, if state.phase == 1 { "4x" } else if state.phase == 2 { "0.25x" } else { "resumed" });
    if state.phase == 1 {
        state.phase = 2;
        state.clock_start = clock;
        state.a_start = a.modelica_target_time;
        state.b_start = b.modelica_target_time;
        set_rate(0.25);
    } else if state.phase == 2 {
        state.phase = 3;
        pause();
        emit(PAUSE_PROBE, 0);
    } else if state.phase == 6 {
        let channels = query("ListTelemetryChannels", #{name: "solverTime"}).channels;
        state.failures.push(t_true(channels.len == 2,
            "both participants publish through the shared telemetry history owner"));
        for channel in channels {
            let history = query("QueryTelemetryHistory", #{key: channel.key, limit: 10});
            let consistent = history.samples.len >= 3;
            for i in 0..history.samples.len {
                let sample = history.samples[i];
                consistent = consistent && abs(sample.v - sample.t) < 1.0e-8;
                if i > 0 { consistent = consistent && sample.t > history.samples[i - 1].t; }
            }
            state.failures.push(t_true(consistent,
                "shared history retains ordered Modelica-time samples for " + channel.name));
        }
        set_rate(1.0);
        return finish(state, state.failures);
    }
    state
}

fn on_start(me, ctx) { this = start_case(false); }
fn on_tick(me, ctx) { this = step_case(this); }
fn on_event(me, evt, ctx) {
    if this.done || evt.name != PAUSE_PROBE { return; }
    let clock = clock_snapshot();
    let rows = query("CosimStatus").entities;
    let a = row(rows, "FeedbackA"); let b = row(rows, "FeedbackB");
    // A dispatched step can finish while transport is paused. Commit that
    // existing transaction before checking that no new transaction starts.
    if a.modelica_is_stepping || b.modelica_is_stepping {
        emit(PAUSE_PROBE, 0);
        return;
    }
    if this.phase == 3 {
        this.phase = 4;
        this.clock_start = clock;
        this.a_start = a.modelica_target_time;
        this.b_start = b.modelica_target_time;
        this.paused_a = a.modelica_current_time;
        this.paused_b = b.modelica_current_time;
        emit(PAUSE_PROBE, 0);
    } else if this.phase == 4 {
        this.failures += [
            t_true(clock.virtual_paused && !clock.simulation_is_running,
                "pause holds the shared simulation transport"),
            t_true(clock.sim_tick == this.clock_start.sim_tick
                && clock.fixed_elapsed_s == this.clock_start.fixed_elapsed_s
                && clock.physics_elapsed_s == this.clock_start.physics_elapsed_s,
                "pause freezes fixed physics time"),
            t_true(a.modelica_current_time == this.paused_a && b.modelica_current_time == this.paused_b,
                "pause freezes both default and slower Modelica participants"),
            t_true(a.modelica_target_time == this.a_start && b.modelica_target_time == this.b_start,
                "paused participants accumulate no wall-time demand")
        ];
        this.phase = 5;
        set_rate(1.0);
        play();
    }
}
