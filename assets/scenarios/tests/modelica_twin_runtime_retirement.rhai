// Production Twin replacement regression. Start with no active Workspace Twin,
// attach with Application scope and Retain reload policy, and supply root_a,
// root_b plus entry_scene and marker_a/marker_b. The Command host signals
// modelica-retirement-scene-ready after each exact fixture marker is projected.
// The owned host config must set experiments.max_parallel = 1.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ReadActiveTwinContract", "ListOpenDocuments", "CompileStatus", "ListCompileCandidates", "RunStatus", "GetExperimentResult"], required_inputs: [] }
}
fn ack(a) {
    if a != () && a.status == "pending" && a.id != () { return command_result(a.id); }
    a
}
fn ready(doc, class) {
    let s = query("CompileStatus", #{doc_id: doc});
    s != () && s.ok != false && s.ast_parsed == true && s.candidates != () && s.candidates.contains(class)
}
fn state(id) {
    let s = query("RunStatus", #{experiment_id: id});
    if s == () || s.ok == false { return "missing"; }
    s.state
}
fn result_is(id, expected) {
    let r = query("GetExperimentResult", #{experiment_id: id, variables: ["x"]});
    if r == () || r.ok == false || r.series == () || r.series["x"] == () || r.series["x"].len() == 0 { return false; }
    let valid = true;
    for x in r.series["x"] { valid = valid && x.is_finite() && (x - expected).abs() < 1.0e-10; }
    valid
}
fn finish(me, failures) {
    if me.done { return me; }
    me.done = true;
    for doc in query("ListOpenDocuments", #{}).open_documents {
        if me.docs.contains(doc.doc_id) { editor::close_scratch(doc.doc_id); }
    }
    report_verdict(failures, "MODELICA TWIN RUNTIME RETIREMENT", "MODELICA_TWIN_RUNTIME_RETIREMENT");
    me
}
fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.phase = "application"; this.reported_phase = "";
    this.docs = []; this.run_a = (); this.run_queued = (); this.run_b = ();
    this.root_a = params.root_a; this.root_b = params.root_b; this.entry = params.entry_scene;
    this.marker_a = params.marker_a; this.marker_b = params.marker_b;
    this.ready_a = false; this.ready_b = false;
    subscribe("modelica-retirement-scene-ready");
    if this.root_a == () || this.root_b == () || this.entry == () || this.root_a == this.root_b || this.marker_a == () || this.marker_b == () {
        this = finish(this, ["distinct Twin roots, entry_scene and exact authored scene markers are required"]); return;
    }
    let twin = query("ReadActiveTwinContract", #{});
    if twin == () || twin.open != false { this = finish(this, ["gate requires initial Application document scope"]); return; }
    let created = editor::new_scratch("model ApplicationProbe Real x(start=7, fixed=true); equation der(x)=0; end ApplicationProbe;", "Application document retirement");
    if created.ok != true { this = finish(this, [created.to_string()]); return; }
    this.app = created.doc_id; this.docs.push(this.app);
}
fn on_event(me, evt, ctx) {
    if evt.name == "modelica-retirement-scene-ready" {
        if evt.value == this.marker_a { this.ready_a = true; }
        if evt.value == this.marker_b { this.ready_b = true; }
    }
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.reported_phase != this.phase {
        print("MODELICA_TWIN_RUNTIME_RETIREMENT phase=" + this.phase);
        this.reported_phase = this.phase;
    }
    if this.ticks > 12000 { this = finish(this, ["Twin runtime retirement timed out at " + this.phase]); return; }
    if this.phase == "application" {
        if !ready(this.app, "ApplicationProbe") { return; }
        let docs = query("ListOpenDocuments", #{}); let app_owner = false;
        for d in docs.open_documents { if d.doc_id == this.app { app_owner = d.runtime_context.kind == "application"; } }
        if !app_owner { this = finish(this, ["scratch admission did not pin Application at allocation"]); return; }
        cmd("CompileModel", #{doc_id: this.app, class: "ApplicationProbe", resume_after_compile: false});
        cmd("CreateTwin", #{path: this.root_a, name: "Runtime owner A", default_scene: this.entry});
        this.phase = "owner_a"; return;
    }
    if this.phase == "owner_a" {
        let twin = query("ReadActiveTwinContract", #{});
        if twin.name != "Runtime owner A" || !this.ready_a { return; }
        let a = editor::new_scratch("model OwnerProbe Real x(start=1, fixed=true); Real z[16](each start=1, each fixed=true); equation der(x)=0; for i in 1:16 loop der(z[i])=-i*z[i]; end for; end OwnerProbe;", "Outgoing owned worker");
        let library = editor::new_scratch("package OutgoingOnly model Part Real y; equation y=42; end Part; end OutgoingOnly;", "Outgoing sibling overlay");
        if a.ok != true || library.ok != true { this = finish(this, ["outgoing documents rejected"]); return; }
        this.a = a.doc_id; this.library = library.doc_id; this.docs.push(this.a); this.docs.push(this.library);
        this.phase = "warm_a"; return;
    }
    if this.phase == "warm_a" {
        if !ready(this.a, "OwnerProbe") || !ready(this.library, "OutgoingOnly.Part") { return; }
        this.warm_ack = cmd("RunExperiment", #{doc_id: this.a, class: "OwnerProbe", t_end: 0.001, n_intervals: 2});
        this.phase = "warm_ack"; return;
    }
    if this.phase == "warm_ack" {
        let r = ack(this.warm_ack); if r == () || r.status == "pending" { return; }
        if r.ok != true || r.data == () || r.data.experiment_id == () { this = finish(this, ["cache warmup rejected: " + r.to_string()]); return; }
        this.warm = r.data.experiment_id; this.phase = "warm_done"; return;
    }
    if this.phase == "warm_done" {
        let s = state(this.warm); if s == "failed" || s == "cancelled" { this = finish(this, ["admitted warmup failed"]); return; }
        if s != "done" { return; }
        this.a_ack = cmd("RunExperiment", #{doc_id: this.a, class: "OwnerProbe", t_end: 10000.0, n_intervals: 200000});
        this.q_ack = cmd("RunExperiment", #{doc_id: this.a, class: "OwnerProbe", t_end: 10000.0, n_intervals: 200000});
        this.app_ack = cmd("RunExperiment", #{doc_id: this.app, class: "ApplicationProbe", t_end: 0.1, n_intervals: 10});
        this.phase = "admission"; return;
    }
    if this.phase == "admission" {
        let a = ack(this.a_ack); let q = ack(this.q_ack); let app = ack(this.app_ack);
        if a == () || q == () || app == () || a.status == "pending" || q.status == "pending" || app.status == "pending" { return; }
        this.closed_app_rejected = app.ok == false && app.status == "rejected" && app.error != ();
        if a.ok != true || q.ok != true || !this.closed_app_rejected {
            this = finish(this, ["expected two owned runs and rejection of the retired Application document: " + [a,q,app].to_string()]); return;
        }
        this.run_a = a.data.experiment_id; this.run_queued = q.data.experiment_id;
        this.phase = "running";
    }
    if this.phase == "running" {
        let a = state(this.run_a); let q = state(this.run_queued);
        if a == "failed" || a == "done" { this = finish(this, ["fixture did not expose an executing native batch before replacement"]); return; }
        if a != "running" || q != "queued" { return; }
        // Delayed live work and the running/queued batch share the outgoing pin.
        cmd("CompileModel", #{doc_id: this.a, class: "OwnerProbe", force: true, resume_after_compile: true});
        cmd("CreateTwin", #{path: this.root_b, name: "Runtime owner B", default_scene: this.entry});
        this.phase = "owner_b"; return;
    }
    if this.phase == "owner_b" {
        let twin = query("ReadActiveTwinContract", #{}); if twin.name != "Runtime owner B" || !this.ready_b { return; }
        let b = editor::new_scratch("model OwnerProbe Real x(start=3, fixed=true); equation der(x)=0; end OwnerProbe;", "Replacement same-name source");
        let bad = editor::new_scratch("model ForeignOverlayProbe OutgoingOnly.Part foreign; end ForeignOverlayProbe;", "Foreign overlay negative");
        if b.ok != true || bad.ok != true { this = finish(this, ["replacement documents rejected"]); return; }
        this.b = b.doc_id; this.bad = bad.doc_id; this.docs.push(this.b); this.docs.push(this.bad);
        this.phase = "successor"; return;
    }
    if this.phase == "successor" {
        if !ready(this.b, "OwnerProbe") || !ready(this.bad, "ForeignOverlayProbe") { return; }
        cmd("CompileModel", #{doc_id: this.bad, class: "ForeignOverlayProbe", force: true});
        this.b_ack = cmd("RunExperiment", #{doc_id: this.b, class: "OwnerProbe", t_end: 0.1, n_intervals: 10});
        this.phase = "successor_ack"; return;
    }
    if this.phase == "successor_ack" {
        let r = ack(this.b_ack); if r == () || r.status == "pending" { return; }
        if r.ok != true || r.data == () || r.data.experiment_id == () { this = finish(this, ["replacement run rejected: " + r.to_string()]); return; }
        this.run_b = r.data.experiment_id; this.phase = "verify"; return;
    }
    if this.phase != "verify" { return; }
    let b = state(this.run_b);
    if b == "failed" || b == "cancelled" { this = finish(this, ["successor run failed"]); return; }
    if b != "done" { return; }
    let bad = query("CompileStatus", #{doc_id: this.bad}); if bad.error_message == () { return; }
    let old = query("ListCompileCandidates", #{doc_id: this.a});
    let closed_app = query("ListCompileCandidates", #{doc_id: this.app});
    this = finish(this, [
        t_true(state(this.run_a) == "cancelled", "executing outgoing native batch terminates as cancelled"),
        t_true(state(this.run_queued) == "cancelled", "queued outgoing native batch is withdrawn"),
        t_true(result_is(this.run_b, 3.0), "replacement successor completes from exact same-name source after slot release"),
        t_true(this.closed_app_rejected, "a retired Application document cannot submit cached native work"),
        t_true(old.ok == false && old.status == "not_found", "outgoing document and late compile cannot revive"),
        t_true(closed_app.ok == false && closed_app.status == "not_found", "loose Application editor document retires on Twin replacement"),
        t_true(bad.is_compiled == false && bad.error_message != (), "retained outgoing sibling cannot satisfy replacement imports")
    ]);
}
