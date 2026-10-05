// Production command/query regression for same-name Modelica documents.
// Attach through RunScenarioAsset to a loaded scene root in an owned UI-capable Modelica host.

import "lunco://scripting/tools/modelica_editor" as modelica_editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["CompileStatus", "QueryExperimentBounds", "RunStatus", "GetExperimentResult"],
       required_inputs: [] }
}

fn finish(me, failures) {
    if me.done { return me; }
    me.done = true;
    // Every admitted solve is terminal before normal cleanup. Timeout cleanup
    // uses the scratch-document owner's cancellation boundary.
    if me.a != () { modelica_editor::close_scratch(me.a); }
    if me.b != () { modelica_editor::close_scratch(me.b); }
    report_verdict(failures, "MODELICA RUN ADMISSION ISOLATION", "MODELICA_RUN_ADMISSION_ISOLATION");
    me
}

fn resolved_ack(ack) {
    if ack != () && ack.status == "pending" && ack.id != () {
        return command_result(ack.id);
    }
    ack
}

fn on_start(me, params) {
    this.a = (); this.b = (); this.done = false; this.ticks = 0;
    this.phase = "prepare"; this.ack_a = (); this.ack_b = ();
    this.run_a = (); this.run_b = ();
    let a = modelica_editor::new_scratch(
        "model AdmissionProbe Real x(start=1, fixed=true); equation der(x)=0; annotation(experiment(StopTime=2, Interval=0.01)); end AdmissionProbe;",
        "Admission ownership A");
    if a == () || a.ok != true { this = finish(this, ["first scratch document was rejected"]); return; }
    this.a = a.doc_id;
    let b = modelica_editor::new_scratch(
        "model AdmissionProbe Real x(start=2, fixed=true); equation der(x)=0; end AdmissionProbe;",
        "Admission ownership B");
    if b == () || b.ok != true { this = finish(this, ["second scratch document was rejected"]); return; }
    this.b = b.doc_id;
}

fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["admission isolation timed out"]); return; }
    if this.phase == "prepare" {
        let a = query("CompileStatus", #{doc_id: this.a});
        let b = query("CompileStatus", #{doc_id: this.b});
        if a == () || b == () || a.ast_parsed != true || b.ast_parsed != true { return; }
        if a.candidates == () || b.candidates == ()
            || !a.candidates.contains("AdmissionProbe") || !b.candidates.contains("AdmissionProbe") { return; }
        this.ack_a = cmd("RunExperiment", #{doc_id: this.a, class: "AdmissionProbe"});
        this.phase = "admit_a";
    }
    if this.phase == "admit_a" {
        let ack = resolved_ack(this.ack_a);
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true || ack.data == () || ack.data.experiment_id == () {
            this = finish(this, ["first run did not return an exact experiment id: " + ack.to_string()]); return;
        }
        this.run_a = ack.data.experiment_id;
        let bounds = query("QueryExperimentBounds", #{doc_id: this.b, class: "AdmissionProbe"});
        if bounds == () || bounds.ok == false || bounds.classes == () || bounds.classes.len() != 1 {
            this = finish(this, ["second document bounds were unavailable: " + bounds.to_string()]); return;
        }
        if bounds.classes[0].resolved_bounds.t_end != 1.0 || bounds.classes[0].source != "fallback_1s" {
            this = finish(this, ["unannotated same-name document inherited another document's bounds"]); return;
        }
        this.ack_b = cmd("RunExperiment", #{doc_id: this.b, class: "AdmissionProbe", dt: 0.01});
        this.phase = "admit_b";
    }
    if this.phase == "admit_b" {
        let ack = resolved_ack(this.ack_b);
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true || ack.data == () || ack.data.experiment_id == () {
            this = finish(this, ["second run did not return an exact experiment id: " + ack.to_string()]); return;
        }
        this.run_b = ack.data.experiment_id;
        this.phase = "results";
    }
    if this.phase != "results" { return; }
    let a = query("RunStatus", #{experiment_id: this.run_a});
    let b = query("RunStatus", #{experiment_id: this.run_b});
    if a == () || b == () { return; }
    if a.state == "failed" || b.state == "failed" || a.state == "cancelled" || b.state == "cancelled" {
        this = finish(this, ["same-name admitted solve failed or was cancelled"]); return;
    }
    if a.state != "done" || b.state != "done" { return; }
    let failures = [];
    for pair in [[this.run_a, 1.0], [this.run_b, 2.0]] {
        let result = query("GetExperimentResult", #{experiment_id: pair[0], variables: ["x"]});
        let valid = result != () && result.ok != false && result.series != ()
            && result.series["x"] != () && result.series["x"].len() > 0;
        if valid {
            for value in result.series["x"] { valid = valid && value.is_finite() && (value - pair[1]).abs() < 1.0e-10; }
        }
        failures.push(t_true(valid, "each run retains its admitted document source"));
    }
    this = finish(this, failures);
}
