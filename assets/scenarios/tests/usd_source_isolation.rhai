// Read-only production gate: source previews must not replace a running Twin.
const TEST_KIND = "editor";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["PhysicsPerformance", "GetReadiness", "RuntimeDiagnostics", "GetToolLibrary", "ListOpenDocuments", "InspectUsdViewport", "InspectUsdSelection"],
       required_inputs: [] }
}

fn no_runtime_errors() {
    for finding in query("RuntimeDiagnostics").findings {
        if finding.severity == "error" { return false; }
    }
    true
}

fn same_physics_topology(before, after) {
    before.bodies == after.bodies && before.colliders == after.colliders
        && before.dynamic_bodies == after.dynamic_bodies && before.joints == after.joints
}

fn on_start(me, ctx) {
    this.done = false; this.elapsed = 0.0; this.checks = [];
    this.native_path = ctx.expected_native_path; this.selection_path = ctx.selection_path;
    this.waiting_for = "source projection";
    this.source_ok = false; this.preview_ready = false; this.matches = 0;
    this.document = (); this.document_count = 0;
    this.owner = query("GetToolLibrary", #{ name: "runtime_ui" }).active_twin;
    this.physics = query("PhysicsPerformance");
    subscribe("TASK_FAILED");
    cmd("ActivatePerspective", #{ id: "editor" });
    this.result = cmd("OpenUsdSourceDocument", #{ source: ctx.source });
}

fn source_ready(me) {
    let result = command_result(this.result.id);
    if result.status == "pending" { return false; }
    this.open_status = result.status;
    if result.status != "applied" { return true; }
    let documents = query("ListOpenDocuments").open_documents;
    let physics = query("PhysicsPerformance");
    this.matches = 0; this.document = ();
    for doc in documents {
        if doc.origin.kind == "file" && doc.origin.path == this.native_path {
            this.matches += 1; this.document = doc.doc_id;
        }
    }
    for preview in query("InspectUsdViewport").previews {
        if preview.doc_id == this.document {
            this.preview_ready = preview.projection_ready;
            this.preview = preview.preview; this.view = preview.views[0].view;
        }
    }
    this.document_count = documents.len();
    this.preview_ready && physics.step_number > this.physics.step_number
}

fn verify_source(me) {
    let physics = query("PhysicsPerformance");
    let owner = query("GetToolLibrary", #{ name: "runtime_ui" }).active_twin;
    this.source_ok = this.open_status == "applied" && this.matches == 1 && this.preview_ready;
    this.checks.push(t_true(this.open_status == "applied", "exact USD source opens successfully"));
    this.checks.push(t_true(this.matches == 1 && this.preview_ready, "one document owns the settled source preview"));
    this.checks.push(t_true(owner == this.owner, "source preview preserves the active Twin"));
    this.checks.push(t_true(same_physics_topology(this.physics, physics), "preview does not enter scene physics"));
    this.checks.push(t_true(physics.step_number > this.physics.step_number
        && query("GetReadiness").faulted == false && no_runtime_errors(),
        "physics continues without runtime errors during source preparation"));
}

fn source_conforms(me) { this.source_ok }

fn select_source(me) {
    this.waiting_for = "preview-scoped prim selection";
    cmd("FocusUsdPreviewView", #{ view: this.view });
    cmd("SelectUsdPrim", #{ preview: this.preview, path: this.selection_path });
}

fn selected_source(selection, document, path) {
    selection != () && selection.ok != false && selection.focused == true && selection.doc_id == document
        && selection.primary != () && selection.primary.path == path
        && selection.selected != () && selection.selected.len() == 1
}

fn selection_ready(me) {
    selected_source(query("InspectUsdSelection", #{ preview: this.preview }), this.document, this.selection_path)
}

fn verify_selection_and_reject(me) {
    this.checks.push(t_true(selected_source(query("InspectUsdSelection", #{ preview: this.preview }), this.document, this.selection_path),
        "source prim remains selectable through its preview lease"));
    this.waiting_for = "invalid-source rejection";
    this.result = cmd("OpenUsdSourceDocument", #{ source: "notes.txt" });
}

fn rejection_complete(me) { command_result(this.result.id).status != "pending" }

fn verify_rejection(me) {
    let result = command_result(this.result.id);
    let documents = query("ListOpenDocuments").open_documents;
    let physics = query("PhysicsPerformance");
    let owner = query("GetToolLibrary", #{ name: "runtime_ui" }).active_twin;
    this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "unsupported source is rejected without crashing"));
    this.checks.push(t_true(documents.len() == this.document_count && owner == this.owner
        && same_physics_topology(this.physics, physics) && query("GetReadiness").faulted == false
        && no_runtime_errors(),
        "rejected source preserves documents, Twin, and non-faulted physics topology"));
    this.done = true; report_verdict(this.checks, "USD SOURCE ISOLATION", "USD_SOURCE_ISOLATION");
}

fn task(me, ctx) {
    seq([
        wait_until(Fn("source_ready")), once(Fn("verify_source")),
        check(Fn("source_conforms")), once(Fn("select_source")),
        wait_until(Fn("selection_ready")), once(Fn("verify_selection_and_reject")),
        wait_until(Fn("rejection_complete")), once(Fn("verify_rejection")),
    ])
}

fn failed_state(state, message) {
    state.checks.push(t_true(false, message + ": " + state.waiting_for));
    state.done = true; report_verdict(state.checks, "USD SOURCE ISOLATION", "USD_SOURCE_ISOLATION");
    state
}

fn on_event(me, evt, ctx) {
    if evt.name == "TASK_FAILED" && !this.done { this = failed_state(this, "source isolation task failed"); }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 { this = failed_state(this, "source isolation timeout"); }
}
