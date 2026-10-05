// Production bounds admission regression. Attach through RunScenarioAsset to
// a loaded scene root in an owned UI-capable Modelica host.
import "lunco://scripting/tools/modelica_editor" as modelica_editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["CompileStatus", "QueryExperimentBounds", "RunStatus", "ListRuns"],
       required_inputs: [] }
}

fn finish(state, failures) {
    if state.done { return state; }
    state.done = true;
    if state.good != () { modelica_editor::close_scratch(state.good); }
    if state.bad != () { modelica_editor::close_scratch(state.bad); }
    report_verdict(failures, "MODELICA RUN BOUNDS ADMISSION", "MODELICA_RUN_BOUNDS_ADMISSION");
    state
}

fn resolved_ack(ack) {
    if ack != () && ack.status == "pending" && ack.id != () { return command_result(ack.id); }
    ack
}

fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.good = (); this.bad = ();
    this.phase = "prepare"; this.index = 0; this.ack = (); this.run = (); this.failures = [];
    this.requests = [
        #{ fields: #{t_end: 0.0}, diagnostic: "horizon" },
        #{ fields: #{t_start: 2.0, t_end: 1.0}, diagnostic: "horizon" },
        #{ fields: #{dt: 0.0}, diagnostic: "dt" },
        #{ fields: #{dt: -1.0}, diagnostic: "dt" },
        #{ fields: #{dt: -1.0, n_intervals: 10}, diagnostic: "dt" },
        #{ fields: #{dt: 1.0e-300}, diagnostic: "output grid" },
        #{ fields: #{n_intervals: 0}, diagnostic: "n_intervals" },
        #{ fields: #{n_intervals: 4294967295}, diagnostic: "n_intervals" },
        #{ fields: #{tolerance: 0.0}, diagnostic: "tolerance" },
        #{ fields: #{h0: -1.0}, diagnostic: "h0" }
    ];
    let good = modelica_editor::new_scratch(
        "model BoundsProbe Real x(start=1, fixed=true); equation der(x)=0; end BoundsProbe;", "Bounds admission valid");
    if good == () || good.ok != true { this = finish(this, ["valid scratch creation failed"]); return; }
    this.good = good.doc_id;
    let bad = modelica_editor::new_scratch(
        "model BadCount Real x(start=1, fixed=true); equation der(x)=0; annotation(experiment(StopTime=1, NumberOfIntervals=1.5)); end BadCount;",
        "Bounds admission invalid annotation");
    if bad == () || bad.ok != true { this = finish(this, ["invalid-count scratch creation failed"]); return; }
    this.bad = bad.doc_id;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["bounds admission timed out"]); return; }
    if this.phase == "prepare" {
        let good = query("CompileStatus", #{doc_id: this.good});
        let bad = query("CompileStatus", #{doc_id: this.bad});
        if good == () || bad == () || good.ast_parsed != true || bad.ast_parsed != true
            || good.candidates == () || bad.candidates == ()
            || !good.candidates.contains("BoundsProbe") || !bad.candidates.contains("BadCount") { return; }
        let annotation = query("QueryExperimentBounds", #{doc_id: this.bad, class: "BadCount"});
        this.failures.push(t_true(annotation != () && annotation.ok == false
            && annotation.error != () && annotation.error.to_string().contains("NumberOfIntervals"),
            "invalid authored count reports the owning diagnostic: " + annotation.to_string()));
        this.phase = "invalid";
    }
    if this.phase == "invalid" {
        if this.ack == () {
            let fields = this.requests[this.index].fields;
            fields.doc_id = this.good; fields.class = "BoundsProbe";
            this.ack = cmd("RunExperiment", fields);
        }
        let ack = resolved_ack(this.ack);
        if ack == () || ack.status == "pending" { return; }
        this.failures.push(t_true(ack.ok == false && ack.status == "rejected" && ack.error != ()
            && ack.error.contains(this.requests[this.index].diagnostic),
            "explicit invalid bounds reject visibly: " + ack.to_string()));
        this.index += 1; this.ack = ();
        if this.index < this.requests.len() { return; }
        this.ack = cmd("RunExperiment", #{doc_id: this.bad, class: "BadCount"});
        this.phase = "authored";
    }
    if this.phase == "authored" {
        let ack = resolved_ack(this.ack);
        if ack == () || ack.status == "pending" { return; }
        this.failures.push(t_true(ack.ok == false && ack.status == "rejected" && ack.error != ()
            && ack.error.contains("NumberOfIntervals"), "authored count cannot fall back during admission"));
        let good_runs = query("ListRuns", #{doc_id: this.good});
        let bad_runs = query("ListRuns", #{doc_id: this.bad});
        this.failures.push(t_true(good_runs != () && good_runs.count == 0 && bad_runs != () && bad_runs.count == 0,
            "rejected bounds do not register experiments"));
        this.ack = cmd("RunExperiment", #{doc_id: this.good, class: "BoundsProbe", t_end: 0.1, n_intervals: 10});
        this.phase = "valid";
    }
    if this.phase == "valid" {
        let ack = resolved_ack(this.ack);
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true || ack.data == () || ack.data.experiment_id == () {
            this.failures.push("valid successor was not admitted: " + ack.to_string());
            this = finish(this, this.failures); return;
        }
        this.run = ack.data.experiment_id; this.phase = "result";
    }
    if this.phase == "result" {
        let result = query("RunStatus", #{experiment_id: this.run});
        if result == () || result.state == "pending" || result.state == "queued" || result.state == "running" { return; }
        this.failures.push(t_true(result.state == "done", "a valid successor still completes: " + result.to_string()));
        this = finish(this, this.failures);
    }
}
