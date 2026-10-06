// Run on an owned client with its remote scene already admitted. Attach in
// Application scope with Retain reload policy. Supply remote_path (a cached
// remote .mo), application_path (a loose .mo), their distinct class names, and
// dirty_source (valid replacement source for the remote class). Both files must
// be unopened in this session. No path spelling is used to infer their owners.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CompileStatus", "GetDocumentSource"], required_inputs: []}
}
fn ready(doc, class) {
    let s = query("CompileStatus", #{doc_id: doc});
    s != () && s.ok != false && s.ast_parsed == true && s.candidates != () && s.candidates.contains(class)
}
fn finish(me, failures) {
    if me.done { return me; }
    me.done = true;
    report_verdict(failures, "MODELICA REMOTE FILE RETIREMENT", "MODELICA_REMOTE_FILE_RETIREMENT");
    me
}
fn on_start(me, params) {
    let ids = [];
    let listed = query("ListOpenDocuments", #{});
    if listed != () && listed.open_documents != () {
        for doc in listed.open_documents { ids.push(doc.doc_id); }
    }
    cmd("OpenFile", #{path: params.remote_path});
    cmd("OpenFile", #{path: params.application_path});
    this = #{done: false, ticks: 0, phase: "opening", initial_ids: ids,
      remote_path: params.remote_path, remote_class: params.remote_class,
      application_class: params.application_class, dirty_source: params.dirty_source,
      remote: 0, application: 0, owner: (), dirty_generation: 0, after_leave: 0}
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 4000 { this = finish(this, ["remote file gate timed out at " + this.phase]); return; }
    if this.phase == "opening" {
        let listed = query("ListOpenDocuments", #{});
        if listed == () || listed.open_documents == () { return; }
        for doc in listed.open_documents {
            if this.initial_ids.contains(doc.doc_id) { continue; }
            if ready(doc.doc_id, this.remote_class) {
                if doc.runtime_context.kind != "replicated_twin" { this = finish(this, ["remote file did not retain actual mounted owner: " + doc.to_string()]); return; }
                this.remote = doc.doc_id; this.owner = doc.runtime_context;
            }
            if ready(doc.doc_id, this.application_class) {
                if doc.runtime_context.kind != "application" { this = finish(this, ["loose file negative was scoped to a scene: " + doc.to_string()]); return; }
                this.application = doc.doc_id;
            }
        }
        if this.remote == 0 || this.application == 0 { return; }
        cmd("CompileModel", #{doc_id: this.remote, class: this.remote_class, force: true});
        cmd("CompileModel", #{doc_id: this.application, class: this.application_class, force: true});
        this.phase = "compiled"; return;
    }
    if this.phase == "compiled" {
        let remote = query("CompileStatus", #{doc_id: this.remote});
        let application = query("CompileStatus", #{doc_id: this.application});
        if remote.error_message != () || application.error_message != () { this = finish(this, ["fixture compile failed: " + [remote, application].to_string()]); return; }
        if remote.is_compiled != true || application.is_compiled != true { return; }
        let source = query("GetDocumentSource", #{doc_id: this.remote});
        this.dirty_generation = source.generation;
        editor::replace_source(this.remote, this.dirty_source, source.generation);
        this.phase = "dirty"; return;
    }
    if this.phase == "dirty" {
        let source = query("GetDocumentSource", #{doc_id: this.remote});
        if source.dirty != true || source.generation == this.dirty_generation { return; }
        cmd("OpenFile", #{path: this.remote_path});
        cmd("CompileModel", #{doc_id: this.remote, class: this.remote_class, force: true, resume_after_compile: true});
        cmd("LeaveServer", #{});
        this.phase = "retired"; return;
    }
    if this.phase == "retired" {
        let remote = query("CompileStatus", #{doc_id: this.remote});
        if remote.is_compiled == true || remote.is_compiling == true || remote.running == true { return; }
        // A post-withdrawal reopen cannot turn retained dirty remote source
        // into application-owned source merely because its cache file remains.
        cmd("OpenFile", #{path: this.remote_path});
        cmd("CompileModel", #{doc_id: this.remote, class: this.remote_class, force: true, resume_after_compile: true});
        this.phase = "verify"; return;
    }
    if this.phase != "verify" { return; }
    this.after_leave += 1;
    if this.after_leave < 20 { return; }
    let retained = ();
    let listed = query("ListOpenDocuments", #{});
    for doc in listed.open_documents { if doc.doc_id == this.remote { retained = doc; } }
    let source = query("GetDocumentSource", #{doc_id: this.remote});
    let remote = query("CompileStatus", #{doc_id: this.remote});
    let application = query("CompileStatus", #{doc_id: this.application});
    this = finish(this, [
        t_true(retained != () && retained.runtime_context == this.owner, "dirty cached source retains exact retired owner"),
        t_true(source.source == this.dirty_source && source.dirty == true, "reopen rejects retagging and preserves unsaved source"),
        t_true(remote.is_compiled == false && remote.is_compiling == false && remote.running == false, "late file and compile work cannot revive retired participant"),
        t_true(application.is_compiled == true, "application file participation survives remote withdrawal")
    ]);
}
