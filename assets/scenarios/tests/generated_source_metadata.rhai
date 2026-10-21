// Generic generated-network API contract. Supply the composed network_root.
// @timing presentation
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GeneratedModelicaSource", "CompileStatus"], required_inputs: []}
}
fn on_start(me, ctx) { this.elapsed=0.0; this.done=false; }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += execution_context().delta_seconds;
    let data=query("GeneratedModelicaSource", #{network_root:ctx.network_root, include_source:false});
    if data.networks==() || data.networks.len()!=1 || data.networks[0].document_source_matches!=true {
        if this.elapsed>20.0 { throw("current generated document was not admitted"); }
        return;
    }
    let network=data.networks[0];
    let compile=query("CompileStatus", #{doc_id:network.document_id});
    if compile.ast_parsed!=true {
        if this.elapsed>20.0 { throw("generated document did not parse"); }
        return;
    }
    let missing=query("GeneratedModelicaSource", #{network_root:ctx.network_root+"/Missing", include_source:false});
    let bad_flag=query("GeneratedModelicaSource", #{network_root:ctx.network_root, include_source:"false"});
    let bad_root=query("GeneratedModelicaSource", #{network_root:12, include_source:false});
    report_verdict([
        t_true(!("source" in network), "metadata read omits the program text"),
        t_true(network.document_id>0 && network.document_generation>=0
            && network.document_source_matches, "metadata identifies a source-current registry document"),
        t_true(compile.doc_id==network.document_id && compile.candidates.contains(network.model_name),
            "the linked document compiles the generated root class"),
        t_true(missing.networks.is_empty(), "an absent network does not invent metadata"),
        t_true(bad_flag.ok==false && bad_flag.error.contains("must be a boolean"),
            "invalid source-selection type rejects visibly"),
        t_true(bad_root.ok==false && bad_root.error.contains("must be a string"),
            "invalid network identity type rejects visibly"),
    ], "GENERATED SOURCE METADATA", "GENERATED_SOURCE_METADATA");
    this.done=true;
}
