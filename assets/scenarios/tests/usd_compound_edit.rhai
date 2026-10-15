const TEST_KIND = "headless";
const ROOT = "/ModelicaRuntimeContracts";
const TARGET = ROOT + "/Compound";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this = #{ done: false, phase: 0, ticks: 0, doc: () };
}

fn finish(checks) {
    report_verdict(checks, "USD COMPOUND EDIT", "USD_COMPOUND_EDIT");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.done = true;
    this.ticks += 1;
    if this.ticks > 120 { finish([t_true(false, "view projection timed out")]); return; }
    if this.phase == 0 {
        let root = query("QueryUsdPrim", #{ path: ROOT });
        if root == () || root.ok == false {
            finish([t_true(false, "the scene document must be available")]);
            return;
        }
        let doc = root.doc_id;
        let created = cmd("ApplyUsdTransientOps", #{ doc_id: doc, ops: [
            #{ AddPrim: #{ edit_target: "@view@", parent_path: ROOT, name: "Compound",
                type_name: "Xform", reference: (), reference_prim_path: () } },
        ] });
        if created.ok != true { finish([t_true(false, created.error)]); return; }
        let applied = cmd("ApplyUsdTransientOps", #{ doc_id: doc, ops: [
            #{ SetTranslate: #{ edit_target: "@view@", path: TARGET, value: [1.0, 2.0, 3.0] } },
            #{ SetRotate: #{ edit_target: "@view@", path: TARGET, value: [10.0, 20.0, 30.0] } },
            #{ SetTranslate: #{ edit_target: "@view@", path: TARGET, value: [4.0, 5.0, 6.0] } },
            #{ SetAttribute: #{ edit_target: "@view@", path: TARGET, name: "visibility",
                type_name: "token", value: "\"inherited\"" } },
        ] });
        if applied.ok != true { finish([t_true(false, applied.error)]); return; }
        this.doc = doc;
        this.phase = 1;
        this.done = false;
        return;
    }
    if find(TARGET) < 0 { this.done = false; return; }
    let doc = this.doc;
    let before = query("QueryUsdPrim", #{ doc_id: doc, path: TARGET,
        attrs: ["xformOp:translate", "xformOpOrder", "visibility"] });
    if before == () || before.ok == false || before.attrs == () {
        finish([t_true(false, "the projected view must expose its attributes")]); return;
    }
    let generation = usd_document_generation(doc);
    let rejected = cmd("ApplyUsdTransientOps", #{ doc_id: doc, parent_gen: generation, ops: [
        #{ SetTranslate: #{ edit_target: "@view@", path: TARGET, value: [99.0, 99.0, 99.0] } },
        #{ SetTranslate: #{ edit_target: "@view@", path: "/Missing", value: [0.0, 0.0, 0.0] } },
    ] });
    let after = query("QueryUsdPrim", #{ doc_id: doc, path: TARGET,
        attrs: ["xformOp:translate"] });
    finish([
        t_true(before.attrs["xformOp:translate"] == [4.0, 5.0, 6.0],
            "later operations observe the current native composition"),
        t_true(before.attrs["xformOpOrder"] == ["xformOp:translate", "xformOp:rotateXYZ"]
            && before.attrs["visibility"] == "inherited", "ordered transforms and typed attributes survive"),
        t_true(rejected.ok == false && rejected.error != "",
            "a trailing invalid operation rejects terminally"),
        t_true(usd_document_generation(doc) == generation
            && after.attrs["xformOp:translate"] == before.attrs["xformOp:translate"],
            "rejection publishes neither a partial layer nor a generation"),
    ]);
}
