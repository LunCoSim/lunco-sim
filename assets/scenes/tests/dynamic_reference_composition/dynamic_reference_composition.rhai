fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [],
        entity_reads: [],
        entity_writes: [],
        query_reads: ["ListOpenDocuments", "QueryUsdPrim", "SceneTestParameters"],
        required_inputs: [],
    }
}

fn fail(detail) {
    report_verdict(
        [t_true(false, detail)],
        "DYNAMIC REFERENCE COMPOSITION",
        "DYNAMIC_REFERENCE_COMPOSITION",
    );
}

fn find_scene_document(scene_name) {
    let documents = query("ListOpenDocuments", #{});
    if documents == () || documents.ok == false || documents.open_documents == () {
        return ();
    }
    for document in documents.open_documents {
        if document.kind == "usd" && document.origin != ()
            && document.origin.path != ()
            && document.origin.path.ends_with(scene_name) {
            return document.doc_id;
        }
    }
    ()
}

fn on_start(me, ctx) {
    this.started = false;
    this.done = false;
    this.reject_case = false;
    this.scene_name = "allow.usda";
    this.doc = ();

    let parameters = query("SceneTestParameters");
    if parameters == () || parameters.ok != true || parameters.scene == () {
        fail("scene-test parameters did not identify the active fixture");
        return;
    }
    this.reject_case = parameters.scene.index_of("reject.usda") >= 0;
    if this.reject_case {
        this.scene_name = "reject.usda";
        expect_runtime_fault("usd-reference-admission");
    }
    this.doc = find_scene_document(this.scene_name);
    if this.doc == () {
        let path = "twin://dynamic_reference_composition/" + this.scene_name;
        let opened = cmd("OpenFile", #{ path: path });
        if opened == () || opened.ok != true {
            let detail = if opened == () { "no command result" } else { opened.error };
            fail("the exact test USD document could not be opened: " + detail);
        }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    if this.doc == () {
        this.doc = find_scene_document(this.scene_name);
        if this.doc == () {
            if sim_tick() > 120 {
                this.done = true;
                fail("OpenFile did not publish the exact fixture document through ListOpenDocuments");
            }
            return;
        }
    }

    if !this.started {
        let root = query("QueryUsdPrim", #{ doc_id: this.doc, path: "/World" });
        if root == () || root.ok == false || root.generation == () {
            this.done = true;
            fail("the opened fixture document has no writable /World prim");
            return;
        }
        let admission = cmd("ApplyUsdOps", #{
            doc_id: this.doc,
            parent_gen: root.generation,
            label: "Exercise dynamic USD composition admission",
            ops: [
                #{ AddPrim: #{
                    edit_target: "@runtime@",
                    parent_path: "/World",
                    name: "Partial",
                    type_name: "Xform",
                    reference: "twin://dynamic_reference_composition/partial.usda",
                    reference_prim_path: "",
                }},
            ],
        });
        if admission == () || admission.ok != true {
            let detail = if admission == () { "no command result" } else { admission.error };
            this.done = true;
            fail("dynamic partial-stage reference was not admitted for preparation: " + detail);
            return;
        }
        this.started = true;
    }

    if this.reject_case { return; }

    let partial = find_path("/World/Partial");
    if partial < 0 {
        if sim_tick() > 120 {
            this.done = true;
            fail("allow_partial did not project the valid root of the incomplete referenced stage");
        }
        return;
    }

    let inspected = query("QueryUsdPrim", #{ path: "/World/Partial" });
    let absent_child = find_path("/World/Partial/Missing");
    let rejected_scene = "twin://dynamic_reference_composition/partial.usda";
    expect_scene_load_failure(
        rejected_scene,
        "unresolved USD composition dependencies",
    );
    this.done = true;
    report_verdict([
        t_true(inspected != () && inspected.ok != false,
            "allow_partial projected the valid referenced root"),
        t_true(absent_child < 0,
            "the unresolved nested reference did not fabricate a child prim"),
    ], "DYNAMIC REFERENCE COMPOSITION", "DYNAMIC_REFERENCE_COMPOSITION");
    load_scene(rejected_scene);
}
