// Production Modelica Editor checkpoint contract through the authoring facade.
import "lunco://scripting/tools/modelica_editor" as editor;
import "lunco://scripting/tools/authoring_session" as authoring;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetDocumentSource", "CompileStatus"], required_inputs: []}
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA EDITOR CHECKPOINT", "MODELICA_EDITOR_CHECKPOINT");
    state
}
fn on_start(me, params) {
    this = #{done: false, ticks: 0, doc: (), phase: "parse", checks: []};
    this.source = "model EditorCheckpoint output Real answer(unit=\"K\")=3; end EditorCheckpoint;";
    let created = editor::new_scratch(this.source, "Editor checkpoint source");
    if created.ok != true { this = finish(this, [created.to_string()]); return; }
    this.doc = created.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3600 { this = finish(this, ["Editor checkpoint timed out at " + this.phase]); return; }
    let compile = query("CompileStatus", #{doc_id: this.doc});
    if this.phase == "parse" {
        if compile == () || compile.ast_parsed != true { return; }
        let source = query("GetDocumentSource", #{doc_id: this.doc});
        this.checks.push(t_true(source.ok == () && compile.ok == ()
            && source.doc_id == this.doc && source.source == this.source,
            "successful source and compile queries expose native data"));
        let inspected = editor::inspect(this.doc);
        if inspected == () || inspected.ok != true {
            this = finish(this, ["Editor inspection did not admit native query data: " + inspected.to_string()]); return;
        }
        this.checks.push(t_true(inspected.source.source == this.source
            && inspected.compile.ast_parsed == true && inspected.generation == source.generation,
            "Editor inspection combines current source and compile facts"));
        let before = authoring::checkpoint("modelica", this.doc);
        this.checks.push(t_true(before.ok == false && before.stage == "compile",
            "parsed source alone is not a verified compiled checkpoint"));
        this.pending = cmd("CompileModel", #{doc_id: this.doc, class: "EditorCheckpoint"});
        this.phase = "compile"; return;
    }
    if this.phase == "compile" {
        let dispatched = ack(this.pending);
        if dispatched != () && dispatched.ok == false { this = finish(this, [dispatched.to_string()]); return; }
        if compile == () || compile.is_compiling == true || compile.state == "idle" { return; }
        let checked = authoring::checkpoint("modelica", this.doc);
        if checked.ok != true || checked.stage != "verified" {
            this = finish(this, ["compiled authoring checkpoint failed: " + checked.to_string()]); return;
        }
        this.checks.push(t_true(checked.inspection.compile.is_compiled == true
            && checked.generation == checked.inspection.source.generation,
            "authoring checkpoint reports the installed current compiled model"));
        this.pending = editor::replace_source(this.doc,
            "model EditorCheckpoint Real x = ; end EditorCheckpoint;", checked.generation);
        this.phase = "invalid"; return;
    }
    if this.phase == "invalid" {
        let edited = ack(this.pending);
        if edited == () || edited.status == "pending" { return; }
        if edited.ok != true { this = finish(this, [edited.to_string()]); return; }
        if compile == () || compile.ast_parsed != false { return; }
        let checked = authoring::checkpoint("modelica", this.doc);
        this.checks.push(t_true(checked.ok == false && checked.stage == "compile"
            && checked.inspection.compile.ast_parsed == false,
            "invalid current source rejects the previously compiled checkpoint"));
        let closed = editor::close_scratch(this.doc);
        if closed.ok != true { this = finish(this, [closed.to_string()]); return; }
        this.closed = this.doc; this.doc = (); this.phase = "closed"; return;
    }
    if this.phase == "closed" {
        let checked = authoring::checkpoint("modelica", this.closed);
        this.checks.push(t_true(checked.ok == false && checked.error != (),
            "retired document preserves its owning query rejection"));
        this = finish(this, this.checks);
    }
}
