fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.ticks = 0;
    let document = ();
    for entry in query("ListOpenDocuments").open_documents {
        if entry.kind == "usd" && entry.origin.kind == "file"
            && entry.origin.path.ends_with("scenario_source_failure.usda") {
            document = entry.doc_id;
        }
    }
    if document == () {
        this.done = true;
        report_verdict(["source-failure scene document is missing"],
            "SCENARIO SOURCE FAILURE", "SCENARIO_SOURCE_FAILURE");
        return;
    }
    expect_runtime_fault("scenario-source-failed");
    let result = assembly_edit::attach_program(document, #{
        edit_target: "@runtime@", host_path: "/World/FailureTarget", name: "MissingSource",
        source_asset: "lunco://scenarios/tests/nonexistent_source_admission_probe.rhai",
        inputs: [], outputs: [], realtime_safe: false,
    });
    if result.ok != true {
        this.done = true;
        report_verdict(["missing-source attachment was rejected before loader admission"],
            "SCENARIO SOURCE FAILURE", "SCENARIO_SOURCE_FAILURE");
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 300 {
        this.done = true;
        report_verdict(["missing scenario source did not produce a terminal runtime fault"],
            "SCENARIO SOURCE FAILURE", "SCENARIO_SOURCE_FAILURE");
    }
}
