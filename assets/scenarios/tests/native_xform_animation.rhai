// Attach through RunScenarioAsset to the settled replacement fixture.
const ROOT = "/World/NativeXformAnimation";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument", "GetReadiness"], required_inputs: []}
}
fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@runtime@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn order(path, value) {
    #{SetAttribute: #{edit_target: "@runtime@", path: path, name: "xformOpOrder",
        type_name: "token[]", value: value}}
}
fn sample(path, name, time, value) {
    #{SetTimeSample: #{edit_target: "@runtime@", path: path, name: name,
        type_name: "float", time: time, value: value}}
}
fn on_start(me, ctx) {
    this.phase = 0; this.doc = (); this.pending = (); this.checks = [];
    this.started = sim_tick(); this.wait_until = 0;
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 600 {
        this.checks.push(t_true(false, "native transform animation timed out at phase " + this.phase));
        report_verdict(this.checks, "NATIVE XFORM ANIMATION", "NATIVE_XFORM_ANIMATION");
        this.phase = 99; return;
    }
    if this.doc == () {
        for document in query("ListOpenDocuments").open_documents {
            if document.active && document.kind == "usd" { this.doc = document.doc_id; }
        }
        if this.doc == () { return; }
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if this.phase == 3 {
            this.checks.push(t_true(result.ok == false && result.error != (),
                "malformed transform order is terminally rejected"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected transform order cannot mutate the document"));
            this.checks.push(t_true(query("GetReadiness").faulted == false,
                "rejected transform input keeps the runtime non-faulted"));
            report_verdict(this.checks, "NATIVE XFORM ANIMATION", "NATIVE_XFORM_ANIMATION");
            this.phase = 99;
        } else {
            if result.ok != true || result.status != "applied" { throw("transform fixture failed: " + result.to_string()); }
            this.wait_until = sim_tick() + 30;
        }
        this.pending = ();
    }
    if this.phase == 99 || sim_tick() < this.wait_until { return; }
    if this.phase == 0 {
        let rotating = ROOT + "/Suffixed";
        let inert = ROOT + "/Unordered";
        let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native ordered animation", ops: [
            add("/World", "NativeXformAnimation", "Scope"),
            add(ROOT, "Suffixed", "Cube"),
            sample(rotating, "xformOp:rotateZ:spin", 0.0, "0"),
            sample(rotating, "xformOp:rotateZ:spin", 1.0, "90"),
            order(rotating, "[\"xformOp:rotateZ:spin\"]"),
            add(ROOT, "Unordered", "Cube"),
            sample(inert, "xformOp:rotateZ", 0.0, "0"),
            sample(inert, "xformOp:rotateZ", 1.0, "90"),
            order(inert, "[]")
        ]});
        if queued == () || queued.id == () { throw("transform fixture was not admitted"); }
        this.pending = queued.id; this.phase = 1; return;
    }
    if this.phase == 1 {
        let rotating = find(ROOT + "/Suffixed"); let inert = find(ROOT + "/Unordered");
        if rotating < 0 || inert < 0 { return; }
        track_entity_read(rotating); track_entity_read(inert);
        if get(rotating, "Mesh3d") == () || get(inert, "Mesh3d") == () { return; }
        this.checks.push(t_true(get(rotating, "UsdAnimated") != (),
            "native ordered suffix operation admits animation"));
        this.checks.push(t_true(get(inert, "UsdAnimated") == (),
            "sampled operation omitted from xformOpOrder remains inert"));
        this.checks.push(t_true(abs(get(rotating, "Transform.rotation.w")) < 0.8 &&
            abs(get(inert, "Transform.rotation.w")) > 0.99,
            "production sampler rotates the suffix stack and preserves the unordered identity"));
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (),
            op: order(ROOT + "/Suffixed", "[]")});
        if queued == () || queued.id == () { throw("transform order edit was not admitted"); }
        this.pending = queued.id; this.phase = 2; return;
    }
    if this.phase == 2 {
        let rotating = find(ROOT + "/Suffixed");
        if rotating < 0 { return; }
        track_entity_read(rotating);
        this.checks.push(t_true(get(rotating, "UsdAnimated") == (),
            "live order edit retires the animation marker"));
        this.checks.push(t_true(get(rotating, "Mesh3d") != (),
            "live order edit preserves the projected geometry"));
        this.generation = assembly_edit::describe(this.doc).generation;
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: #{SetAttribute: #{edit_target: "@runtime@", path: ROOT + "/Suffixed",
                name: "xformOpOrder", type_name: "bool", value: "true"}}});
        if queued == () || queued.id == () { throw("negative transform edit was not admitted"); }
        this.pending = queued.id; this.phase = 3;
    }
}
