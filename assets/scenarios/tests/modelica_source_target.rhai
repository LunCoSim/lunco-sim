// Source admission rejects absent targets and retires a replaced live model.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "CosimStatus"], required_inputs: []}
}
const ROOT = "/ModelicaRuntimeContracts";
const LIBRARY = "lunco://models/tests/SourceLibrary.mo";
const TARGET = "lunco://models/tests/SourceTargetProbe.mo";
fn attach(doc, name, source) {
    assembly_edit::attach_program(doc, #{edit_target: "@runtime@", host_path: ROOT,
        name: name, source_asset: source, inputs: [],
        outputs: [#{name: "sample", type_name: "double", connections: []}],
        realtime_safe: false})
}
fn replace_source(doc, source) {
    cmd("ApplyUsdOp", #{doc_id: doc, op: #{SetAttribute: #{
        edit_target: "@runtime@", path: ROOT + "/Replacement", name: "info:sourceAsset",
        type_name: "asset", value: "@" + source + "@"}}})
}
fn row(rows, path) {
    for item in rows { if item.name == path { return item; } }
    ()
}
fn finish(state, failures) {
    state.done = true;
    report_verdict(failures, "MODELICA SOURCE TARGET", "MODELICA_SOURCE_TARGET");
    state
}
fn on_start(me, ctx) {
    this = #{done: false, ticks: 0, phase: "admission", doc: (), failures: []};
    this.participants = query("CosimStatus").synchronization.active_participants;
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.title == "modelica_runtime_contracts" { this.doc = doc.doc_id; }
    }
    if this.doc == () { this = finish(this, ["runtime contract document is missing"]); return; }
    for spec in [["NoTarget", LIBRARY], ["Replacement", TARGET]] {
        let admitted = attach(this.doc, spec[0], spec[1]);
        if admitted == () || admitted.ok != true {
            this = finish(this, ["program attachment failed: " + admitted.to_string()]); return;
        }
    }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3000 { this = finish(this, ["source target admission timed out at " + this.phase]); return; }
    let status = query("CosimStatus");
    if status == () || status.entities == () { return; }
    let fresh = row(status.entities, ROOT + "/NoTarget");
    let replacement = row(status.entities, ROOT + "/Replacement");
    if this.phase == "cleanup" {
        if fresh != () || replacement != ()
            || status.synchronization.active_participants != this.participants { return; }
        this.failures.push(t_true(status.synchronization.active_participants == this.participants,
            "removed source fixtures release live participants"));
        this = finish(this, this.failures); return;
    }
    if fresh == () || replacement == () { return; }
    if this.phase == "admission" {
        if !fresh.status.starts_with("Error:") || replacement.status != "Running" { return; }
        this.failures += [
            t_true(fresh.status.contains("contains no runnable model, block, or class"),
                "library-only source is rejected by its admission owner"),
            t_true(fresh.modelica_communication_period_secs == () && fresh.modelica_var_count.to_float() == 0.0,
                "missing target creates no dummy solver participant"),
            t_true(replacement.outputs.sample == 3.0, "a valid source runs its actual selected class")
        ];
        let changed = replace_source(this.doc, LIBRARY);
        if changed == () || changed.ok != true {
            this = finish(this, ["source replacement failed: " + changed.to_string()]); return;
        }
        this.phase = "replacement"; return;
    }
    if this.phase == "replacement" {
        if !replacement.status.starts_with("Error:") { return; }
        this.failures += [
        t_true(replacement.status.contains("contains no runnable model, block, or class"),
            "replacement source uses the same target admission diagnostic"),
        t_true(replacement.modelica_communication_period_secs == () && !replacement.modelica_is_stepping,
            "replacement rejection retires the installed solver participant"),
        t_true(replacement.modelica_var_count.to_float() == 0.0,
            "replacement rejection clears the installed solver observations")
        ];
        let changed = replace_source(this.doc, TARGET);
        if changed == () || changed.ok != true {
            this = finish(this, ["source recovery failed: " + changed.to_string()]); return;
        }
        this.phase = "recovery"; return;
    }
    if replacement.status != "Running" { return; }
    this.failures += [
        t_true(replacement.outputs.sample == 3.0, "a valid replacement recovers after source rejection"),
        t_true(replacement.modelica_var_count.to_float() == 1.0 && replacement.modelica_error == (),
            "the recovered participant exposes only its new checked observations")
    ];
    let removed = cmd("ApplyUsdOps", #{doc_id: this.doc, ops: [
        #{RemovePrim: #{edit_target: "@runtime@", path: ROOT + "/NoTarget"}},
        #{RemovePrim: #{edit_target: "@runtime@", path: ROOT + "/Replacement"}}
    ]});
    if removed == () || removed.ok != true { this = finish(this, ["fixture removal failed"]); return; }
    this.phase = "cleanup";
}
