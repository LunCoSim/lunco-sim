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
fn hidden(path) {
    let id = find(path);
    if id < 0 { return false; }
    track_entity_read(id);
    return get(id, "Visibility").to_string().contains("Hidden");
}
fn visibility_write(id, value) {
    #{target:id, writes:[#{property:"visibility", value:value}]}
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
            add(ROOT + "/Hidden", "LocallyHidden", "Cube"), attr(ROOT + "/Hidden/LocallyHidden", "visibility", "invisible"),
            add(ROOT + "/Hidden", "SuppressedGuide", "Cube"), attr(ROOT + "/Hidden/SuppressedGuide", "purpose", "guide"),
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
        this.checks.push(t_true(mesh(ROOT + "/Hidden/Child") != () && hidden(ROOT + "/Hidden/Child"),
            "initially hidden children retain prepared geometry without rendering"));
        this.checks.push(t_true(mesh(ROOT + "/Guide/Shown") != (), "native guide visibility can explicitly show guide geometry"));
        this.checks.push(t_true(mesh(ROOT + "/Guide/Suppressed") != () && hidden(ROOT + "/Guide/Suppressed"),
            "nearest purpose visibility can suppress a guide without discarding geometry"));
        this.checks.push(t_true(mesh(ROOT + "/Hidden/ShownGuide") != () && hidden(ROOT + "/Hidden/ShownGuide"),
            "overall invisibility dominates visible guide opinions"));
        this.child = find(ROOT + "/Hidden/Child");
        this.child_mesh = mesh(ROOT + "/Hidden/Child").to_string();
        this.unrelated = find(ROOT + "/Ordinary");
        this.hidden_root = find(ROOT + "/Hidden");
        track_entity_write(this.hidden_root);
        track_entity_write(this.child);
        let generation = assembly_edit::describe(this.doc).generation;
        let reveal = cmd("SetSceneView", #{batches:[visibility_write(this.hidden_root, #{Bool:#{value:true}})]});
        this.checks.push(t_true(reveal.ok && !hidden(ROOT + "/Hidden/Child") && !hidden(ROOT + "/Hidden/ShownGuide") &&
            hidden(ROOT + "/Hidden/LocallyHidden") && hidden(ROOT + "/Hidden/SuppressedGuide"),
            "one root reveal respects authored hidden descendants and native guide purpose"));
        let hide = cmd("SetSceneView", #{batches:[visibility_write(this.hidden_root, #{Bool:#{value:false}}),
            visibility_write(this.child, #{Bool:#{value:true}})]});
        this.checks.push(t_true(hide.ok && hidden(ROOT + "/Hidden/Child") && hidden(ROOT + "/Hidden/ShownGuide"),
            "a descendant override cannot punch through a hidden ancestor"));
        let cleared = cmd("SetSceneView", #{batches:[visibility_write(this.hidden_root, "Clear"), visibility_write(this.child, "Clear")]});
        this.checks.push(t_true(cleared.ok && hidden(ROOT + "/Hidden/Child") &&
            assembly_edit::describe(this.doc).generation == generation && mesh(ROOT + "/Hidden/Child").to_string() == this.child_mesh,
            "clearing a root override restores authored subtree visibility with no document edit or mesh replacement"));
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (), op: attr(ROOT + "/Hidden", "visibility", "inherited")});
        if queued == () || queued.id == () { throw("visibility edit was not admitted"); }
        this.pending = queued.id; this.phase = 2; return;
    }
    if this.phase == 2 {
        this.checks.push(t_true(!hidden(ROOT + "/Hidden/Child") && !hidden(ROOT + "/Hidden/ShownGuide"),
            "live ancestor visibility edit reveals ordinary and explicitly visible guide geometry"));
        this.checks.push(t_true(find(ROOT + "/Hidden/Child") == this.child &&
            mesh(ROOT + "/Hidden/Child").to_string() == this.child_mesh &&
            find(ROOT + "/Ordinary") == this.unrelated,
            "authored visibility retains the geometry and unrelated entity identities"));
        let generation = assembly_edit::describe(this.doc).generation;
        let rejected = cmd("SetSceneView", #{batches:[
            visibility_write(this.child, #{Bool:#{value:false}}),
            visibility_write(this.unrelated, #{Text:#{value:"invalid"}})
        ]});
        this.checks.push(t_true(rejected.ok == false && !hidden(ROOT + "/Hidden/Child"),
            "an invalid visibility batch leaves all targets unchanged"));
        let hidden_result = cmd("SetSceneView", #{batches:[visibility_write(this.child, #{Bool:#{value:false}})]});
        this.checks.push(t_true(hidden_result.ok && hidden(ROOT + "/Hidden/Child") &&
            mesh(ROOT + "/Hidden/Child").to_string() == this.child_mesh,
            "runtime visibility hides immediately without replacing the mesh"));
        let cleared = cmd("SetSceneView", #{batches:[visibility_write(this.child, "Clear")]});
        this.checks.push(t_true(cleared.ok && !hidden(ROOT + "/Hidden/Child") &&
            assembly_edit::describe(this.doc).generation == generation,
            "clearing runtime visibility restores the authored value without editing USD"));
        let generic_hide = cmd("SetObjectProperty", #{entity_id:this.hidden_root,property:"visible",value:"false",intent:"Interactive"});
        let invalid_value = cmd("SetObjectProperty", #{entity_id:this.hidden_root,property:"visible",value:"maybe",intent:"Interactive"});
        this.checks.push(t_true(generic_hide.ok && !invalid_value.ok && hidden(ROOT + "/Hidden/Child"),
            "generic visibility uses the same root owner and rejects invalid boolean values"));
        let clear_root = cmd("SetSceneView", #{batches:[visibility_write(this.hidden_root,"Clear")]});
        this.checks.push(t_true(clear_root.ok && !hidden(ROOT + "/Hidden/Child") &&
            assembly_edit::describe(this.doc).generation == generation,
            "the typed view command clears the generic runtime opinion"));
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
        this.failed_guide = failed;
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (),
            op: attr(ROOT + "/Guide", "guideVisibility", "visible")});
        if queued == () || queued.id == () { throw("visibility repair was not admitted"); }
        this.pending = queued.id; this.phase = 6; return;
    }
    if this.phase == 6 {
        this.checks.push(t_true(find(ROOT + "/Guide") == this.failed_guide &&
            get(this.failed_guide, "UsdSceneProjectionFailed") == () &&
            !hidden(ROOT + "/Guide/Shown") && hidden(ROOT + "/Guide/Suppressed"),
            "correcting an invalid live visibility opinion restores the same projected subtree"));
        report_verdict(this.checks, "NATIVE VISIBILITY", "NATIVE_VISIBILITY");
        this.phase = 99;
    }
}
