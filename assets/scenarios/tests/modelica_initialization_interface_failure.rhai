import "lunco://scripting/tools/assembly_edit" as assembly;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ListOpenDocuments"], required_inputs: []}
}
fn on_start(me, ctx) {
    expect_runtime_fault("modelica-compile-failed");
    let doc = ();
    for item in query("ListOpenDocuments").open_documents {
        if item.kind == "usd" && item.title == "modelica_initialization_interface_failure" {
            doc = item.doc_id;
        }
    }
    if doc == () { throw("interface rejection fixture document is missing"); }
    let result = assembly::attach_program(doc, #{edit_target: "@runtime@",
        host_path: "/World", name: "Rejected", source_asset: "lunco://models/tests/InitialConstantOutput.mo",
        inputs: [], outputs: [#{name: "calibration", type_name: "double", connections: []}],
        realtime_safe: false});
    if result.ok != true { throw(result.to_string()); }
}
