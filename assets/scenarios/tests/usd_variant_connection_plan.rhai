// Production admission of typed connections authored in selected USD variants.
const ROOT = "/RockerBogie";
const OWNER = "/RockerBogie{generation=solar}";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim", "InspectUsdViewport", "InspectUsdDocument"], required_inputs: []}
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn plan(source, sink) {
    [#{SetAttribute: #{edit_target: "@root@", path: OWNER, name: source,
                      type_name: "double", value: "1"}},
     #{SetConnection: #{edit_target: "@root@", path: OWNER, name: sink,
                       type_name: "double", sources: [ROOT + "." + source]}}]
}
fn finish(state, checks) {
    state.done = true;
    if state.source != () { assembly_edit::discard_document(state.source); assembly_edit::close_document(state.source); }
    report_verdict(checks, "USD VARIANT CONNECTION PLAN", "USD_VARIANT_CONNECTION_PLAN");
    state
}
fn on_start(me, ctx) {
    this.done = false; this.elapsed = 0.0; this.phase = "open"; this.source = (); this.checks = [];
    let listed = query("ListOpenDocuments", #{});
    if listed == () || listed.open_documents == () { this = finish(this, ["document inventory failed: " + listed.to_string()]); return; }
    this.initial = []; for document in listed.open_documents { this.initial.push(document.doc_id); }
    cmd("OpenUsdSourceDocument", #{source: "lunco://vessels/rovers/rocker_bogie.usda"});
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 { this = finish(this, ["variant plan timed out at " + this.phase]); return; }
    if this.phase == "open" {
        for document in query("ListOpenDocuments", #{}).open_documents {
            if document.kind == "usd" && !this.initial.contains(document.doc_id) {
                this.source = document.doc_id;
                this.request = assembly_edit::variant(this.source, "@view@", ROOT, "generation", "solar", ());
                this.phase = "select"; return;
            }
        }
        return;
    }
    let result = ack(this.request);
    if result == () || result.status == "pending" { return; }
    if this.phase == "select" {
        if result.error != () { this = finish(this, ["variant selection failed: " + result.to_string()]); return; }
        this.request = assembly_edit::batch(this.source, "Selected variant plan",
            plan("outputs:batch_probe", "inputs:batch_sink"), ());
        this.phase = "positive"; return;
    }
    if this.phase == "positive" {
        if result.error != () { this = finish(this, ["selected variant plan rejected: " + result.to_string()]); return; }
        this.admitted_generation = query("InspectUsdDocument", #{doc_id: this.source, path: ROOT}).generation;
        this.phase = "observe"; return;
    }
    if this.phase == "observe" {
        let viewport = query("InspectUsdViewport", #{});
        if viewport.ok == false { this = finish(this, ["viewport inspection failed: " + viewport.to_string()]); return; }
        let settled = false;
        for preview in viewport.previews {
            if preview.doc_id == this.source && preview.projection_ready
                && preview.projected_generation == this.admitted_generation { settled = true; }
        }
        if !settled { return; }
        let prim = query("QueryUsdPrim", #{doc_id: this.source, path: ROOT, attrs: ["outputs:batch_probe"]});
        this.checks.push(t_true(prim.ok != false && prim.attrs["outputs:batch_probe"] == 1.0,
            "selected variant plan publishes its composed source"));
        this.request = assembly_edit::variant(this.source, "@view@", ROOT, "generation", "none", ());
        this.phase = "deselect"; return;
    }
    if this.phase == "deselect" {
        if result.error != () { this = finish(this, ["variant deselection failed"]); return; }
        this.request = assembly_edit::batch(this.source, "Unselected variant source",
            plan("outputs:unselected_probe", "inputs:unselected_sink"), ());
        this.phase = "negative"; return;
    }
    this.checks.push(t_true(result.error != () && result.error.contains("missing property"),
        "an unselected variant cannot fabricate a composed connection source"));
    this = finish(this, this.checks);
}
