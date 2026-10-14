// Attach to the settled replacement fixture through RunScenarioAsset.
// Standard defaultPrim is owned by the root layer, regardless of edit scope.
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "InspectUsdDocument"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.doc = ();
    this.phase = 0;
    this.pending = ();
    this.generation = ();
    this.checks = [];
    this.started = sim_tick();
}

fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 300 { throw("defaultPrim authoring timed out"); }
    if this.doc == () {
        let docs = query("ListOpenDocuments", #{});
        for document in docs.open_documents {
            if document.active && document.kind == "usd" { this.doc = document.doc_id; }
        }
        if this.doc == () { return; }
        this.generation = assembly_edit::describe(this.doc).generation;
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if this.phase <= 2 {
            this.checks.push(t_true(result.ok == false && result.to_string().contains("requires @root@"),
                "non-root defaultPrim produces a terminal owner diagnostic"));
            this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
                "rejected defaultPrim does not mutate the document"));
        } else {
            this.checks.push(t_true(result.ok == true && result.status == "applied",
                "root helper commits through the typed document owner"));
            let description = assembly_edit::describe(this.doc);
            this.checks.push(t_true(description.metadata.defaultPrim.composed.value == "World",
                "inspection reads the native root defaultPrim"));
        }
        this.pending = ();
    }
    if this.phase < 2 {
        let target = if this.phase == 0 { "@runtime@" } else { "@view@" };
        let queued = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: this.generation,
            op: #{SetDefaultPrim: #{edit_target: target, default_prim: "World"}}});
        if queued == () || queued.id == () { throw("defaultPrim rejection was not admitted"); }
        this.pending = queued.id;
        this.phase += 1;
        return;
    }
    if this.phase == 2 {
        let publication = model_authoring::publish_component(this.doc, "/World", "@runtime@",
            "native_default_prim_unused.usda", #{});
        this.checks.push(t_true(publication.ok == false && publication.to_string().contains("choose @root@"),
            "source publication rejects a runtime edit target before producing a plan"));
        this.checks.push(t_true(assembly_edit::describe(this.doc).generation == this.generation,
            "rejected source publication does not mutate the document"));
        let queued = assembly_edit::default_prim(this.doc, "/World", this.generation);
        if queued == () || queued.id == () { throw("root defaultPrim was not admitted"); }
        this.pending = queued.id;
        this.phase = 3;
        return;
    }
    report_verdict(this.checks, "NATIVE DEFAULT PRIM", "NATIVE_DEFAULT_PRIM");
    this.phase = 99;
}
