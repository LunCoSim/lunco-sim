// Production Save All scope regression. Start without an active Workspace
// Twin in a UI-capable Modelica/FileOps host. Attach with Application scope
// and Retain reload policy. Supply distinct root_a/root_b directories and
// entry_scene, an API-authored entry present in both fixture roots.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ReadActiveTwinContract", "ListTwin", "ListOpenDocuments", "GetDocumentSource", "CompileStatus"],
      required_inputs: []}
}
fn record(doc) {
    let listed = query("ListOpenDocuments", #{});
    if listed == () || listed.open_documents == () { return (); }
    for entry in listed.open_documents { if entry.doc_id == doc { return entry; } }
    ()
}
fn snapshot(doc) {
    let entry = record(doc);
    let source = query("GetDocumentSource", #{doc_id: doc});
    if entry == () || source == () || source.ok != true { return (); }
    #{source: source.source, generation: source.generation, dirty: source.dirty,
      origin: source.origin, listed_origin: entry.origin, owner: entry.runtime_context}
}
fn ready(doc, class) {
    let status = query("CompileStatus", #{doc_id: doc});
    status != () && status.ok != false && status.ast_parsed == true
        && status.candidates != () && status.candidates.contains(class)
}
fn ack(result) {
    if result != () && result.status == "pending" && result.id != () { return command_result(result.id); }
    result
}
fn finish(me, failures) {
    if me.done { return me; }
    me.done = true;
    for doc in me.docs { editor::close(doc); }
    report_verdict(failures, "MODELICA SAVE ALL SCOPE", "MODELICA_SAVE_ALL_SCOPE");
    me
}
fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.phase = "application"; this.docs = [];
    this.root_a = params.root_a; this.root_b = params.root_b; this.entry = params.entry_scene;
    if this.root_a == () || this.root_b == () || this.entry == ()
        || this.root_a == "" || this.root_b == "" || this.entry == "" || this.root_a == this.root_b {
        this = finish(this, ["two distinct fixture roots and API-authored entry_scene are required"]); return;
    }
    let twin = query("ReadActiveTwinContract", #{});
    if twin == () || twin.open != false { this = finish(this, ["gate requires initial Application document scope"]); return; }
    this.app_source = "model SaveAllApplicationProbe Real x; equation x=11; end SaveAllApplicationProbe;";
    this.a_source = "model SaveAllRetiredAProbe Real x; equation x=22; end SaveAllRetiredAProbe;";
    this.b_source = "model SaveAllCurrentBProbe Real x; equation x=33; end SaveAllCurrentBProbe;";
    let created = editor::new_scratch("model SaveAllApplicationProbe Real x; equation x=1; end SaveAllApplicationProbe;", "SaveAllApplication");
    if created.ok != true { this = finish(this, [created.to_string()]); return; }
    this.app = created.doc_id; this.docs.push(this.app);
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 12000 { this = finish(this, ["Save All scope timed out at " + this.phase]); return; }
    if this.phase == "application" {
        if !ready(this.app, "SaveAllApplicationProbe") { return; }
        let source = snapshot(this.app);
        if source == () || source.owner.kind != "application" || source.origin.kind != "untitled" {
            this = finish(this, ["initial draft did not retain Application/Untitled admission"]); return;
        }
        this.edit_ack = editor::replace_source(this.app, this.app_source, source.generation);
        this.phase = "application_dirty"; return;
    }
    if this.phase == "application_dirty" {
        let edited = ack(this.edit_ack);
        if edited == () || edited.status == "pending" { return; }
        if edited.ok != true { this = finish(this, ["Application edit rejected: " + edited.to_string()]); return; }
        let source = snapshot(this.app);
        if source == () || source.source != this.app_source || source.dirty != true { return; }
        this.app_before = source;
        this.transition_ack = cmd("CreateTwin", #{path: this.root_a, name: "Save All owner A", default_scene: this.entry});
        this.phase = "owner_a"; return;
    }
    if this.phase == "owner_a" {
        let admitted = ack(this.transition_ack);
        if admitted != () && admitted.ok == false { this = finish(this, ["Twin A admission rejected: " + admitted.to_string()]); return; }
        let twin = query("ReadActiveTwinContract", #{});
        if twin == () || twin.name != "Save All owner A" { return; }
        let created = editor::new_scratch("model SaveAllRetiredAProbe Real x; equation x=2; end SaveAllRetiredAProbe;", "SaveAllRetiredA");
        if created.ok != true { this = finish(this, [created.to_string()]); return; }
        this.a = created.doc_id; this.docs.push(this.a);
        this.phase = "draft_a"; return;
    }
    if this.phase == "draft_a" {
        if !ready(this.a, "SaveAllRetiredAProbe") { return; }
        let source = snapshot(this.a);
        if source == () || source.owner.kind != "local_twin" || source.origin.kind != "untitled" {
            this = finish(this, ["A draft did not retain exact Twin/Untitled admission"]); return;
        }
        this.edit_ack = editor::replace_source(this.a, this.a_source, source.generation);
        this.phase = "a_dirty"; return;
    }
    if this.phase == "a_dirty" {
        let edited = ack(this.edit_ack);
        if edited == () || edited.status == "pending" { return; }
        if edited.ok != true { this = finish(this, ["A edit rejected: " + edited.to_string()]); return; }
        let source = snapshot(this.a);
        if source == () || source.source != this.a_source || source.dirty != true { return; }
        this.a_before = source;
        this.transition_ack = cmd("CreateTwin", #{path: this.root_b, name: "Save All owner B", default_scene: this.entry});
        this.phase = "owner_b"; return;
    }
    if this.phase == "owner_b" {
        let admitted = ack(this.transition_ack);
        if admitted != () && admitted.ok == false { this = finish(this, ["Twin B admission rejected: " + admitted.to_string()]); return; }
        let twin = query("ReadActiveTwinContract", #{});
        if twin == () || twin.name != "Save All owner B" { return; }
        if snapshot(this.a) != this.a_before || snapshot(this.app) != this.app_before {
            this = finish(this, ["fixture replacement changed retained dirty A/Application source before Save All"]); return;
        }
        let listing = query("ListTwin", #{});
        if listing == () || listing.open != true || listing.root == () {
            this = finish(this, ["native B fixture root is unavailable through ListTwin"]); return;
        }
        for file in listing.files {
            if file.relative_path.replace("\\", "/") == "SaveAllCurrentB.mo" {
                this = finish(this, ["fixture B already contains gate output SaveAllCurrentB.mo"]); return;
            }
        }
        this.b_root = listing.root.replace("\\", "/");
        let created = editor::new_scratch("model SaveAllCurrentBProbe Real x; equation x=3; end SaveAllCurrentBProbe;", "SaveAllCurrentB");
        if created.ok != true { this = finish(this, [created.to_string()]); return; }
        this.b = created.doc_id; this.docs.push(this.b);
        this.phase = "draft_b"; return;
    }
    if this.phase == "draft_b" {
        if !ready(this.b, "SaveAllCurrentBProbe") { return; }
        let source = snapshot(this.b);
        if source == () || source.owner.kind != "local_twin" || source.owner == this.a_before.owner || source.origin.kind != "untitled" {
            this = finish(this, ["B draft did not receive a distinct current Twin owner"]); return;
        }
        this.b_owner = source.owner;
        this.edit_ack = editor::replace_source(this.b, this.b_source, source.generation);
        this.phase = "b_dirty"; return;
    }
    if this.phase == "b_dirty" {
        let edited = ack(this.edit_ack);
        if edited == () || edited.status == "pending" { return; }
        if edited.ok != true { this = finish(this, ["B edit rejected: " + edited.to_string()]); return; }
        let source = snapshot(this.b);
        if source == () || source.source != this.b_source || source.dirty != true { return; }
        this.save_ack = cmd("SaveAll", #{});
        this.phase = "saved"; return;
    }
    if this.phase == "saved" {
        let saved = ack(this.save_ack);
        if saved != () && saved.ok == false { this = finish(this, ["Save All rejected: " + saved.to_string()]); return; }
        let source = snapshot(this.b);
        if source == () || source.dirty != false || source.origin.kind != "file" { return; }
        if source.origin.path.replace("\\", "/") != this.b_root + "/SaveAllCurrentB.mo"
            || source.owner != this.b_owner || source.source != this.b_source
            || snapshot(this.a) != this.a_before || snapshot(this.app) != this.app_before {
            this = finish(this, ["Save All changed a foreign source or failed to save B into its exact admitted root"]); return;
        }
        this.saved_path = source.origin.path;
        editor::close(this.b);
        this.phase = "closed_b"; return;
    }
    if this.phase == "closed_b" {
        if record(this.b) != () { return; }
        editor::open(this.saved_path);
        this.phase = "reopened_b"; return;
    }
    if this.phase != "reopened_b" { return; }
    let listed = query("ListOpenDocuments", #{});
    if listed == () || listed.open_documents == () { return; }
    for entry in listed.open_documents {
        if entry.doc_id == this.b || entry.origin.kind != "file" || entry.origin.path != this.saved_path { continue; }
        if !ready(entry.doc_id, "SaveAllCurrentBProbe") { return; }
        let persisted = snapshot(entry.doc_id);
        if persisted == () { return; }
        this.docs.push(entry.doc_id);
        this = finish(this, [
            t_true(persisted.source == this.b_source && persisted.dirty == false, "B edited source persists through close and actual file reopen"),
            t_true(persisted.owner == this.b_owner, "saved B file reopens under its exact current Twin owner"),
            t_true(snapshot(this.a) == this.a_before, "retired dirty A keeps source, generation, origin and owner unchanged"),
            t_true(snapshot(this.app) == this.app_before, "Application draft keeps dirty source and pathless origin outside B")
        ]);
        return;
    }
}
