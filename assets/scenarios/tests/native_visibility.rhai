// Attach through RunScenarioAsset to the settled replacement fixture.
const ROOT = "/World/NativeVisibility";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument", "QueryUsdPrim"], required_inputs: []}
}
fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn attr(path, name, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name,
        type_name: "token", value: "\"" + value + "\""}}
}
fn apis(path) {
    #{SetApiSchemas: #{edit_target: "@root@", path: path, schemas: ["VisibilityAPI"]}}
}
fn mesh(path) {
    let id = find(path);
    if id < 0 { return (); }
    track_entity_read(id);
    get(id, "Mesh3d")
}
fn on_start(me, ctx) {
    this.phase = 0; this.doc = (); this.pending = (); this.checks = [];
    this.started = sim_tick(); this.wait_until = 0;
    this.last_failure = ();
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 600 {
        this.checks.push(t_true(false, "native visibility timed out at phase " + this.phase + ": " + this.last_failure));
        report_verdict(this.checks, "NATIVE VISIBILITY", "NATIVE_VISIBILITY");
        this.phase = 99; return;
    }
    if this.doc == () {
        for document in query("ListOpenDocuments", #{}).open_documents {
            if document.active && document.kind == "usd" { this.doc = document.doc_id; }
        }
        if this.doc == () { return; }
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if this.phase == 3 {
            this.checks.push(t_true(result.ok == false, "invalid visibility type is terminally rejected"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected visibility does not mutate the document"));
            this.phase = 4;
        } else {
            if result.ok != true || result.status != "applied" { throw("visibility setup/edit failed: " + result.to_string()); }
            this.wait_until = sim_tick() + 30;
        }
        this.pending = ();
    }
    if sim_tick() < this.wait_until || this.phase == 99 { return; }
    if this.phase == 0 {
        let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native visibility fixture", ops: [
            add("/World", "NativeVisibility", "Xform"), add(ROOT, "Ordinary", "Cube"),
            add(ROOT, "Hidden", "Xform"), attr(ROOT + "/Hidden", "visibility", "invisible"),
            add(ROOT + "/Hidden", "Child", "Cube"), attr(ROOT + "/Hidden/Child", "visibility", "inherited"),
            add(ROOT, "Guide", "Xform"), attr(ROOT + "/Guide", "purpose", "guide"),
            apis(ROOT + "/Guide"), attr(ROOT + "/Guide", "guideVisibility", "visible"),
            add(ROOT + "/Guide", "Shown", "Cube"),
            add(ROOT + "/Guide", "Suppressed", "Cube"), apis(ROOT + "/Guide/Suppressed"),
            attr(ROOT + "/Guide/Suppressed", "guideVisibility", "invisible"),
            add(ROOT + "/Hidden", "ShownGuide", "Cube"), attr(ROOT + "/Hidden/ShownGuide", "purpose", "guide"),
            apis(ROOT + "/Hidden/ShownGuide"), attr(ROOT + "/Hidden/ShownGuide", "guideVisibility", "visible")
        ]});
        if queued == () || queued.id == () { throw("visibility fixture was not admitted"); }
        this.pending = queued.id; this.phase = 1; return;
    }
    if this.phase == 1 {
        if find(ROOT + "/Guide/Shown") < 0 { return; }
        this.checks.push(t_true(mesh(ROOT + "/Ordinary") != (), "ordinary geometry remains visible"));
        this.checks.push(t_true(mesh(ROOT + "/Hidden/Child") == (), "ancestor invisibility prunes inherited children"));
        this.checks.push(t_true(mesh(ROOT + "/Guide/Shown") != (), "native guide visibility can explicitly show guide geometry"));
        this.checks.push(t_true(mesh(ROOT + "/Guide/Suppressed") == (), "nearest purpose visibility can suppress a guide"));
        this.checks.push(t_true(mesh(ROOT + "/Hidden/ShownGuide") == (), "overall invisibility dominates visible guide opinions"));
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (), op: attr(ROOT + "/Hidden", "visibility", "inherited")});
        if queued == () || queued.id == () { throw("visibility edit was not admitted"); }
        this.pending = queued.id; this.phase = 2; return;
    }
    if this.phase == 2 {
        this.checks.push(t_true(mesh(ROOT + "/Hidden/Child") != () && mesh(ROOT + "/Hidden/ShownGuide") != (),
            "live ancestor visibility edit reveals ordinary and explicitly visible guide geometry"));
        this.generation = assembly_edit::describe(this.doc).generation;
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: #{SetAttribute: #{edit_target: "@root@", path: ROOT, name: "visibility", type_name: "bool", value: "true"}}});
        if queued == () || queued.id == () { throw("negative visibility edit was not admitted"); }
        this.pending = queued.id; this.phase = 3;
    }
    if this.phase == 4 {
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (),
            op: attr(ROOT + "/Guide", "guideVisibility", "invalid-visibility")});
        if queued == () || queued.id == () { throw("native visibility error edit was not admitted"); }
        this.pending = queued.id; this.phase = 5; return;
    }
    if this.phase == 5 {
        let failed = find(ROOT + "/Guide");
        let inspected = query("QueryUsdPrim", #{doc_id: this.doc, path: ROOT + "/Guide", topology: true});
        if failed < 0 { return; }
        track_entity_read(failed);
        let error = get(failed, "UsdSceneProjectionFailed");
        this.last_failure = error.to_string();
        if error == () { return; }
        this.checks.push(t_true(error.contains("PurposeVisibility") &&
            get(failed, "Visibility").to_string().contains("Hidden"),
            "native visibility read failure terminates and hides visual projection"));
        this.checks.push(t_true(inspected.topology.diagnostics.len() > 0 && inspected.topology.parts[0].visual == (),
            "inspection preserves unknown visibility with its native diagnostic"));
        report_verdict(this.checks, "NATIVE VISIBILITY", "NATIVE_VISIBILITY");
        this.phase = 99;
    }
}
