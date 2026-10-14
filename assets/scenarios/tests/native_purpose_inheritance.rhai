// Attach through RunScenarioAsset to the settled replacement fixture.
const ROOT = "/World/NativePurposeInheritance";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument", "QueryPhysicsState", "QueryUsdPrim", "GetDiagnostics"], required_inputs: []}
}
fn add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn attr(path, name, kind, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name, type_name: kind, value: value}}
}
fn apis(path, schemas) {
    #{SetApiSchemas: #{edit_target: "@root@", path: path, schemas: schemas}}
}
fn on_start(me, ctx) {
    this.phase = 0; this.doc = (); this.pending = (); this.checks = []; this.started = sim_tick();
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 600 {
        this.checks.push(t_true(false, "native purpose inheritance timed out"));
        report_verdict(this.checks, "NATIVE PURPOSE INHERITANCE", "NATIVE_PURPOSE_INHERITANCE");
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
        if this.phase == 1 {
            if result.ok != true || result.status != "applied" { throw("purpose fixture failed: " + result.to_string()); }
        } else if this.phase == 3 {
            if result.ok != true { throw("purpose lint was rejected"); }
        } else if this.phase == 4 {
            if result.ok != true || result.status != "applied" { throw("purpose edit failed"); }
            this.phase = 5;
        } else {
            this.checks.push(t_true(result.ok == false,
                "malformed purpose type produces a terminal command rejection"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected purpose does not mutate the document"));
            report_verdict(this.checks, "NATIVE PURPOSE INHERITANCE", "NATIVE_PURPOSE_INHERITANCE");
            this.phase = 99;
        }
        this.pending = ();
    }
    if this.phase == 0 {
        let ops = [add("/World", "NativePurposeInheritance", "Xform"),
            apis(ROOT, ["PhysicsRigidBodyAPI", "PhysicsMassAPI"]),
            #{SetPrimKind: #{edit_target: "@root@", path: ROOT, kind: "assembly"}},
            attr(ROOT, "physics:kinematicEnabled", "bool", "true"),
            attr(ROOT, "physics:mass", "float", "10"),
            add(ROOT, "Base", "Cube"), apis(ROOT + "/Base", ["PhysicsCollisionAPI"])];
        for purpose in ["guide", "proxy", "render"] {
            let group = ROOT + "/" + purpose;
            ops.push(add(ROOT, purpose, "Xform"));
            ops.push(attr(group, "purpose", "token", "\"" + purpose + "\""));
            ops.push(add(group, "Shape", "Cube"));
            ops.push(apis(group + "/Shape", ["PhysicsCollisionAPI"]));
        }
        ops.push(attr(ROOT + "/guide/Shape", "size", "double", "100"));
        ops.push(attr(ROOT + "/render/Shape", "size", "double", "200"));
        ops.push(add(ROOT + "/guide", "ExplicitDefault", "Cube"));
        ops.push(attr(ROOT + "/guide/ExplicitDefault", "purpose", "token", "\"default\""));
        ops.push(apis(ROOT + "/guide/ExplicitDefault", ["PhysicsCollisionAPI"]));
        ops.push(add(ROOT + "/guide", "Decoration", "Cube"));
        ops.push(add(ROOT, "Uncovered", "Cube"));
        let queued = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: (), label: "Native purpose fixture", ops: ops});
        if queued == () || queued.id == () { throw("purpose fixture was not admitted"); }
        this.pending = queued.id; this.phase = 1; return;
    }
    if this.phase == 1 {
        let body = find(ROOT);
        if body < 0 { return; }
        track_entity_read(body);
        let state = query("QueryPhysicsState", #{id: body});
        if state == () || state.ok == false || state.physics_state_ready != true { return; }
        let paths = state.collider_source_paths;
        if paths == () { return; }
        this.checks.push(t_true(!paths.contains(ROOT + "/guide/Shape"), "inherited guide is excluded from collision"));
        this.checks.push(t_true(paths.contains(ROOT + "/proxy/Shape") && !paths.contains(ROOT + "/render/Shape"),
            "inherited proxy replaces inherited render collision geometry"));
        this.checks.push(t_true(paths.contains(ROOT + "/guide/ExplicitDefault") && paths.contains(ROOT + "/Base") && paths.len() == 3,
            "explicit default overrides inherited guide and preserves default geometry"));
        let bounds = query("QueryUsdPrim", #{doc_id: this.doc, path: ROOT, collision_bounds: true}).collision_bounds;
        this.checks.push(t_true(bounds != () && bounds.min[0] == -1.0 && bounds.max[0] == 1.0 &&
            state.collider_local_bounds.min[0] == -1.0 && state.collider_local_bounds.max[0] == 1.0,
            "native collision bounds and runtime cooking exclude guide and render extents"));
        let topology = query("QueryUsdPrim", #{doc_id: this.doc, path: ROOT, topology: true}).topology;
        let inherited = #{};
        for part in topology.parts { inherited[part.path] = part.purpose; }
        this.checks.push(t_true(inherited[ROOT + "/guide/Shape"] == "guide" &&
            inherited[ROOT + "/proxy/Shape"] == "proxy" && inherited[ROOT + "/render/Shape"] == "render",
            "topology inspection reports native inherited purpose"));
        this.lint_before = query("GetDiagnostics", #{scope: "loaded_stages"}).revision;
        let queued = cmd("RunLint", #{scope: "loaded_stages"});
        if queued == () || queued.id == () { throw("purpose lint was not admitted"); }
        this.pending = queued.id; this.phase = 3; return;
    }
    if this.phase == 3 {
        let lint = query("GetDiagnostics", #{scope: "loaded_stages"});
        if lint.available != true || lint.complete != true || lint.revision == this.lint_before { return; }
        let decoration = false; let uncovered = false;
        for finding in lint.diagnostics {
            if finding.code != "vehicle-part-collision-contract" { continue; }
            if finding.subject == ROOT + "/guide/Decoration" { decoration = true; }
            if finding.subject == ROOT + "/Uncovered" { uncovered = true; }
        }
        this.checks.push(t_true(!decoration, "lint respects inherited visual-only guide geometry"));
        this.checks.push(t_true(uncovered, "lint diagnoses uncovered default geometry"));
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (),
            op: attr(ROOT + "/guide", "purpose", "token", "\"default\"")});
        if queued == () || queued.id == () { throw("purpose edit was not admitted"); }
        this.pending = queued.id; this.phase = 4; return;
    }
    if this.phase == 5 {
        let body = find(ROOT);
        track_entity_read(body);
        let state = query("QueryPhysicsState", #{id: body});
        let paths = state.collider_source_paths;
        if paths == () || !paths.contains(ROOT + "/guide/Shape") { return; }
        this.checks.push(t_true(paths.len() == 4 && !paths.contains(ROOT + "/render/Shape"),
            "live ancestor purpose edit refreshes the admitted collider"));
        this.generation = assembly_edit::describe(this.doc).generation;
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: attr(ROOT + "/guide", "purpose", "bool", "true")});
        if queued == () || queued.id == () { throw("negative purpose edit was not admitted"); }
        this.pending = queued.id; this.phase = 2;
    }
}
