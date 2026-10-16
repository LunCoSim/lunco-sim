fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim"], required_inputs: []}
}
// Admit the valid fixture, then invalidate the sampling contract through the
// document owner after declaring the exact terminal fault the producer must emit.
fn on_start(me, ctx) {
    let path = "/TerrainSolarVisibility/Power/Panel/Environment";
    let doc = ();
    for item in query("ListOpenDocuments").open_documents {
        if item.kind == "usd" {
            let prim = query("QueryUsdPrim", #{doc_id: item.doc_id, path: path});
            if prim != () && prim.active == true { doc = item.doc_id; break; }
        }
    }
    if doc == () {
        report_verdict(["sampling fixture document is missing"],
            "TERRAIN SOLAR VISIBILITY INVALID", "TERRAIN_SOLAR_VISIBILITY_INVALID");
        return;
    }
    expect_runtime_fault("solar-sample-unavailable");
    cmd("ApplyUsdOp", #{doc_id: doc, op: #{SetAttribute: #{
        edit_target: "@runtime@", path: path, name: "points",
        type_name: "point3f[]", value: "[]",
    }}});
}
