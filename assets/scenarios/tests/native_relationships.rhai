// Typed relationship discovery through the production composed query surface.
const ROOT = "/World/RelationshipProbe";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: []}
}
fn finish(state) {
    state.phase = 99;
    report_verdict(state.checks, "NATIVE RELATIONSHIPS", "NATIVE_RELATIONSHIPS");
    state
}
fn on_start(me, ctx) {
    this.phase = 0; this.doc = (); this.pending = (); this.checks = [];
    this.started = sim_tick();
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.active { this.doc = doc.doc_id; }
    }
    if this.doc == () { throw("native relationship fixture document is missing"); }
}
fn on_tick(me, ctx) {
    if this.phase == 99 { return; }
    if sim_tick() - this.started > 300 {
        this.checks.push(t_true(false, "native relationship query/edit timed out"));
        this = finish(this); return;
    }
    if this.pending != () {
        let result = command_result(this.pending);
        if result.status == "pending" { return; }
        if result.ok != true || result.status != "applied" { throw(result.to_string()); }
        this.pending = ();
    }
    let probe = query("QueryUsdPrim", #{doc_id: this.doc, path: ROOT,
        relationships: true, connections: true});
    if probe.ok == false {
        if probe.to_string().contains("projection is not current") { return; }
        throw(probe.to_string());
    }
    let rels = probe.relationships;
    if this.phase == 0 {
        this.checks += [
            t_true(rels.contains("links:empty"), "an empty authored relationship remains discoverable"),
            t_true(rels["links:empty"] != () && rels["links:empty"].len() == 0,
                "the empty relationship has no fabricated target"),
            t_true(!rels.contains("value") && !rels.contains("inputs:drive") && !rels.contains("outputs:speed"),
                "ordinary and connected attributes are excluded from relationships"),
            t_true(rels["links:child"] == [ROOT + "/Child"], "native relationship targets are preserved"),
            t_true(probe.connections["inputs:drive"] == [ROOT + ".outputs:speed"],
                "attribute connections remain on their own surface")
        ];
        let result = cmd("ApplyUsdOp", #{doc_id: this.doc, parent_gen: (),
            op: #{SetRelationship: #{edit_target: "@runtime@", path: ROOT,
                name: "links:child", targets: []}}});
        if result.id == () { throw(result.to_string()); }
        this.pending = result.id; this.phase = 1; return;
    }
    this.checks += [
        t_true(rels.contains("links:child") && rels["links:child"].len() == 0,
            "clearing targets retains the relationship property"),
        t_true(!rels.contains("inputs:drive") && probe.connections["inputs:drive"] == [ROOT + ".outputs:speed"],
            "a live relationship edit preserves distinct attribute connections")
    ];
    this = finish(this);
}
