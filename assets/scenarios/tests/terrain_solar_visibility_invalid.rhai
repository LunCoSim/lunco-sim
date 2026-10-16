fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments", "QueryUsdPrim", "ReadActiveTwinContract", "GetToolLibrary"], required_inputs: []}
}
// Admit the valid fixture, then invalidate the sampling contract through the
// document owner after declaring the exact terminal fault the producer must emit.
fn on_start(me, ctx) {
    if find_path("/InvalidSolarAdmission") >= 0 {
        expect_runtime_fault("solar-sample-unavailable");
        let twin = query("ReadActiveTwinContract");
        let owner = query("GetToolLibrary", #{name: "runtime_ui"}).active_twin;
        let opened = cmd("OpenTwinScene", #{twin_id: owner, name: twin.name,
            relative_path: "terrain_solar_visibility_invalid.usda"});
        if opened == () || opened.ok != true {
            report_verdict(["negative fixture admission failed: " + opened.to_string()],
                "TERRAIN SOLAR VISIBILITY INVALID", "TERRAIN_SOLAR_VISIBILITY_INVALID");
        }
        return;
    }
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
