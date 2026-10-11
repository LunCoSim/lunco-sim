// A newly authored missing program source must fault and hold simulation.
const TEST_KIND = "headless";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.doc = (); this.opened = false; this.requested = false;
    expect_runtime_fault("scenario-source-load-failed");
}
fn on_tick(me, ctx) {
    if this.requested { return; }
    let documents = query("ListOpenDocuments", #{});
    for document in documents.open_documents {
        if document.origin != () && document.origin.path.ends_with("scenario_source_failure.usda") {
            this.doc = document.doc_id;
        }
    }
    if this.doc == () {
        if !this.opened {
            this.opened = true;
            let opened = cmd("OpenFile", #{path: "lunco://scenes/tests/scenario_source_failure.usda"});
            if opened.ok != true { throw("source failure fixture document could not open"); }
        }
        return;
    }
    let host = query("QueryUsdPrim", #{doc_id: this.doc, path: "/World/SourceHost"});
    if host == () || host.ok == false { throw("source failure fixture has no authored host"); }
    let result = cmd("AttachProgram", #{doc_id: this.doc, spec: #{
        edit_target: "@runtime@", host_path: "/World/SourceHost", name: "MissingSource",
        source_asset: "lunco://scenarios/tests/intentionally_missing_source.rhai",
        inputs: [], outputs: [], realtime_safe: false,
    }});
    if result.ok != true { throw("missing source stimulus was rejected before authoring"); }
    this.requested = true;
}
