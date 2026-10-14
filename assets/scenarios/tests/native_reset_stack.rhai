// Attach through RunScenarioAsset to the settled replacement fixture.
const ROOT = "/World/NativeResetStack";
const CHILD = "/World/NativeResetStack/Reset";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument"], required_inputs: []}
}
fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@runtime@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn attr(path, name, kind, value) {
    #{SetAttribute: #{edit_target: "@runtime@", path: path, name: name, type_name: kind, value: value}}
}
fn reset_order(enabled) {
    if enabled { return "[\"xformOp:translate:discarded\",\"!resetXformStack!\",\"xformOp:translate:after\"]"; }
    "[\"xformOp:translate:discarded\",\"xformOp:translate:after\"]"
}
fn on_start(me, ctx) {
    this.phase = 0; this.doc = (); this.pending = (); this.checks = [];
    this.started = sim_tick(); this.wait_until = 0; this.waiting = "document";
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 600 {
        this.checks.push(t_true(false, "native reset stack timed out at phase " + this.phase + ": " + this.waiting));
        report_verdict(this.checks, "NATIVE RESET STACK", "NATIVE_RESET_STACK");
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
        if this.phase == 4 {
            this.checks.push(t_true(result.ok == false && result.error != (),
                "malformed reset order is terminally rejected"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected reset order does not mutate the document"));
            report_verdict(this.checks, "NATIVE RESET STACK", "NATIVE_RESET_STACK");
            this.phase = 99;
        } else {
            if result.ok != true || result.status != "applied" { throw("reset fixture/edit failed: " + result.to_string()); }
            this.wait_until = sim_tick() + 30;
        }
        this.pending = ();
    }
    if this.phase == 99 || sim_tick() < this.wait_until { return; }
    if this.phase == 0 {
        let ops = [add("/World", "NativeResetStack", "Xform"),
            attr(ROOT, "xformOp:translate", "float3", "(10,0,0)"),
            attr(ROOT, "xformOpOrder", "token[]", "[\"xformOp:translate\"]")];
        for name in ["Reset", "Ordinary"] {
            let path = ROOT + "/" + name;
            ops.push(add(ROOT, name, "Cube"));
            ops.push(attr(path, "xformOp:translate:discarded", "float3", "(100,0,0)"));
            ops.push(attr(path, "xformOp:translate:after", "float3", "(2,0,0)"));
            ops.push(attr(path, "xformOpOrder", "token[]", reset_order(name == "Reset")));
        }
        let parent = ROOT;
        for index in 0..70 {
            let name = "n";
            ops.push(add(parent, name, "Xform"));
            parent += "/" + name;
            ops.push(attr(parent, "xformOp:translate", "float3", "(1,0,0)"));
            ops.push(attr(parent, "xformOpOrder", "token[]", "[\"xformOp:translate\"]"));
        }
        ops.push(add(parent, "Reset", "Cube"));
        this.deep = parent + "/Reset";
        ops.push(attr(this.deep, "xformOp:translate:discarded", "float3", "(100,0,0)"));
        ops.push(attr(this.deep, "xformOp:translate:after", "float3", "(2,0,0)"));
        ops.push(attr(this.deep, "xformOpOrder", "token[]", reset_order(true)));
        let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native reset stack", ops: ops});
        if queued == () || queued.id == () { throw("reset fixture was not admitted"); }
        this.pending = queued.id; this.phase = 1; return;
    }
    let reset = find_path_in_document(this.doc, CHILD);
    let ordinary = find_path_in_document(this.doc, ROOT + "/Ordinary");
    if reset < 0 || ordinary < 0 { this.waiting = "shallow entities"; return; }
    track_entity_read(reset); track_entity_read(ordinary);
    if get(reset, "Mesh3d") == () || get(ordinary, "Mesh3d") == () { this.waiting = "shallow meshes"; return; }
    if this.phase == 1 {
        let deep = find_path_in_document(this.doc, this.deep);
        if deep < 0 { this.waiting = "deep entity"; return; }
        track_entity_read(deep);
        if get(deep, "Mesh3d") == () {
            this.waiting = "deep mesh, failed=" + (get(deep, "UsdSceneProjectionFailed") != ());
            return;
        }
        this.checks.push(t_true(abs(world_pos(reset)[0] - 2.0) < 0.001,
            "middle reset drops earlier operations and ancestor transforms"));
        this.checks.push(t_true(get(reset, "UsdResetXformStack") != (),
            "native reset fact reaches the visual projection"));
        this.checks.push(t_true(abs(world_pos(ordinary)[0] - 112.0) < 0.001,
            "ordinary ordered stack retains the ancestor transform"));
        this.checks.push(t_true(get(ordinary, "UsdResetXformStack") == (),
            "ordinary stack has no reset marker"));
        this.checks.push(t_true(abs(world_pos(deep)[0] - 2.0) < 0.001,
            "deep reset traverses the complete stage ancestry"));
        this.checks.push(t_true(get(deep, "UsdResetXformStack") != (),
            "deep reset retains its native projection fact"));
    } else if this.phase == 2 {
        this.checks.push(t_true(abs(world_pos(reset)[0] - 112.0) < 0.001,
            "removing reset restores the authored parent frame"));
        this.checks.push(t_true(get(reset, "UsdResetXformStack") == (),
            "removing reset retires its marker"));
    } else {
        this.checks.push(t_true(abs(world_pos(reset)[0] - 2.0) < 0.001,
            "restoring reset reanchors the updated projection"));
        this.checks.push(t_true(get(reset, "UsdResetXformStack") != (),
            "restoring reset reinstates its native marker"));
    }
    let op = attr(CHILD, "xformOpOrder", "token[]", reset_order(this.phase == 2));
    if this.phase == 3 {
        this.generation = assembly_edit::describe(this.doc).generation;
        op = attr(CHILD, "xformOpOrder", "bool", "true");
    }
    let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (), op: op});
    if queued == () || queued.id == () { throw("reset edit was not admitted"); }
    this.pending = queued.id; this.phase += 1;
}
