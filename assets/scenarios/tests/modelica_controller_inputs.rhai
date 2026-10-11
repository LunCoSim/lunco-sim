// The API caller supplies the exact authored DescentGuidance source snapshot.
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "SnapshotVariables"], required_inputs: []}
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA CONTROLLER INPUTS", "MODELICA_CONTROLLER_INPUTS");
    state
}
fn on_start(me, ctx) {
    this = #{done: false, ticks: 0, phase: "prepare", doc: (), checks: []};
    if ctx.source == () || ctx.source_uri != "lunco://models/DescentGuidance.mo" {
        this = finish(this, ["the authored DescentGuidance source snapshot is required"]); return;
    }
    let created = editor::new_scratch(ctx.source, "Controller input contract");
    if created.ok != true { this = finish(this, ["source admission failed"]); return; }
    this.doc = created.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["controller input check timed out at " + this.phase]); return; }
    let status = query("CompileStatus", #{doc_id: this.doc});
    if status.state == "error" { this = finish(this, ["compile failed: " + status.to_string()]); return; }
    if this.phase == "prepare" {
        if status.ast_parsed != true || !status.candidates.contains("DescentGuidance") { return; }
        cmd("CompileModel", #{doc_id: this.doc, class: "DescentGuidance", resume_after_compile: true});
        this.phase = "initial"; return;
    }
    let s = query("SnapshotVariables", #{doc_id: this.doc});
    if s.compiled != true || s.variables.throttle_cmd == () { return; }
    if this.phase == "initial" {
        this.checks.push(t_true(s.inputs.kv == 1.2 && s.variables.throttle_cmd == 0.0,
            "authored gain and initial descent clamp are initialized"));
        cmd("SetModelInput", #{doc_id: this.doc, name: "altitude", value: 1.5, producer_id: 91271});
        cmd("SetModelInput", #{doc_id: this.doc, name: "descent_rate", value: -1.0, producer_id: 91271});
        this.phase = "feedback"; return;
    }
    if this.phase == "feedback" {
        if s.inputs.altitude != 1.5 || s.inputs.descent_rate != -1.0 { return; }
        if (s.variables.throttle_cmd - 0.094).abs() > 1.0e-10 { return; }
        this.checks.push(t_true(true, "sensor inputs drive the current control law"));
        cmd("PauseActiveModel", #{doc_id: this.doc});
        cmd("SetModelInput", #{doc_id: this.doc, name: "kv", value: 2.4, producer_id: 91271});
        this.phase = "gain_admit"; return;
    }
    if this.phase == "gain_admit" {
        if s.inputs.kv != 2.4 { return; }
        cmd("ResetActiveModel", #{doc_id: this.doc});
        this.phase = "gain"; return;
    }
    if this.phase == "gain" {
        if s.inputs.kv != 2.4 || s.t != 0.0 { return; }
        this.checks.push(t_true((s.variables.throttle_cmd - 0.134).abs() < 1.0e-10,
            "current gain initializes the algebraic control law: " + s.to_string()));
        cmd("SetModelInput", #{doc_id: this.doc, name: "engage", value: 0.0, producer_id: 91271});
        this.phase = "standby_admit"; return;
    }
    if this.phase == "standby_admit" {
        if s.inputs.engage != 0.0 { return; }
        cmd("ResetActiveModel", #{doc_id: this.doc});
        this.phase = "standby"; return;
    }
    if s.inputs.engage != 0.0 || s.variables.throttle_cmd != 0.0 { return; }
    this.checks.push(t_true(s.variables.throttle_cmd == 0.0, "authority input stops the command"));
    this = finish(this, this.checks);
}
