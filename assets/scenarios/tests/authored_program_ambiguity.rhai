const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["QueryUsdPrim"], required_inputs: []}
}
fn on_start(me, ctx) {
    expect_runtime_fault("usd-program-activation-failed");
    this.started = sim_tick(); this.sent = false;
    let source = query("QueryUsdPrim", #{path: "/World"});
    this.doc = source.doc_id; this.generation = source.document_generation;
}
fn on_tick(me, ctx) {
    if !this.sent && sim_tick() - this.started >= 3 {
        this.sent = true;
        let ops = [];
        for name in ["FirstSupervisor", "SecondSupervisor"] {
            let path = "/World/A/" + name;
            ops.push(#{AddPrim: #{edit_target: "@runtime@", parent_path: "/World/A", name: name,
                type_name: "Scope", reference: (), reference_prim_path: ()}});
            ops.push(#{SetApiSchemas: #{edit_target: "@runtime@", path: path, schemas: ["LunCoProgramAPI"]}});
            ops.push(#{SetAttribute: #{edit_target: "@runtime@", path: path,
                name: "info:implementationSource", type_name: "token", value: "\"sourceAsset\""}});
            ops.push(#{SetAttribute: #{edit_target: "@runtime@", path: path,
                name: "info:sourceAsset", type_name: "asset",
                value: "@lunco://scenarios/tests/paused_object_controller.rhai@"}});
        }
        let submitted = cmd("ApplyUsdOps", #{doc_id: this.doc, parent_gen: this.generation,
            label: "Reject ambiguous executable ownership", ops: ops});
        if submitted.ok != true { throw("Could not author the negative topology: " + submitted); }
    }
    if sim_tick() - this.started >= 120 {
        report_verdict([t_true(false, "ambiguous executable ownership must raise its named runtime fault")],
            "AUTHORED PROGRAM AMBIGUITY", "AUTHORED_PROGRAM_AMBIGUITY");
    }
}
