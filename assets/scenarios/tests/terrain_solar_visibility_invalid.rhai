fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: [], required_inputs: []}
}
// A ready host declares the expected fault before loading malformed physical
// sampling. The scene runner requires the producer's actual terminal fault.
fn on_start(me, ctx) {
    expect_runtime_fault("solar-sample-unavailable");
    cmd("LoadScene", #{path: "lunco://scenes/tests/terrain_solar_visibility_invalid.usda", root_prim: ""});
}
