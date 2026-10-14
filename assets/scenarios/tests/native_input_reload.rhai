const PATH = "/World/NativeInputReloadCube";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim", "GetReadiness"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.doc = (); this.pending = (); this.done = false;
    this.stage = ctx.stage; this.started = sim_tick();
}
fn on_tick(me, ctx) {
    if this.done { return; }
    if sim_tick() - this.started > 600 {
        this.done = true; report_verdict([t_true(false, "native input reload timed out")],
            "NATIVE INPUT RELOAD", "NATIVE_INPUT_RELOAD"); return;
    }
    if this.doc == () {
        for document in query("ListOpenDocuments", #{}).open_documents {
            if document.active && document.kind == "usd" { this.doc = document.doc_id; }
        }
        if this.doc == () { return; }
        if this.stage == "prepare" {
            let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native runtime input", ops: [
                #{AddPrim: #{edit_target: "@runtime@", parent_path: "/World", name: "NativeInputReloadCube",
                    type_name: "Cube", reference: (), reference_prim_path: ()}},
                #{SetAttribute: #{edit_target: "@runtime@", path: PATH, name: "size", type_name: "double", value: "3"}}
            ]});
            if queued == () || queued.id == () { throw("runtime input was not admitted"); }
            this.pending = queued.id;
        }
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if result.ok != true || result.status != "applied" { throw("runtime input failed: " + result.to_string()); }
        this.pending = ();
    }
    let id = find(PATH);
    if id < 0 { return; }
    track_entity_read(id);
    if get(id, "Mesh3d") == () { return; }
    let prim = query("QueryUsdPrim", #{doc_id: this.doc, path: PATH, attrs: ["size"]});
    this.done = true;
    report_verdict([
        t_true(prim != () && prim.ok != false && prim.attrs.size == 3.0,
            "native composed runtime-only geometry survives " + this.stage),
        t_true(query("GetReadiness").faulted == false,
            "native input publication and scene restart remain non-faulted")
    ], "NATIVE INPUT RELOAD", "NATIVE_INPUT_RELOAD");
}
