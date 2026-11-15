// Two boundary inputs on one receiver must each have one visible caption.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[find_path("/World/Network")],entity_reads:[],entity_writes:[],
      query_reads:["GeneratedModelicaSource"],required_inputs:[]}
}
fn on_start(me,ctx) {this.done=false;}
fn on_tick(me,ctx) {
    if this.done {return;}
    let snapshot=query("GeneratedModelicaSource",#{network_root:"/World/Network",include_source:true});
    let network=snapshot.networks[0];let source=network.source;
    let checks=[
        t_true(network.document_source_matches && network.projection_error==(),"generated diagram uses the current admitted source"),
        t_true(source.split("textString=\"u\"").len()==2,"signal boundary caption is drawn exactly once"),
        t_true(source.split("textString=\"sample valid\"").len()==2,"validity boundary caption is drawn exactly once"),
        t_true(network.boundary_inputs.contains("signal") && network.boundary_inputs.contains("valid"),"both scalar bindings remain in the generated interface"),
        t_true(abs(get(find_path("/World/Network"),"measurement")-2.0)<1.0e-12,"diagram bundling preserves the receiver equation")
    ];
    this.done=true;report_verdict(checks,"GENERATED BOUNDARY GRAPHICS","GENERATED_BOUNDARY_GRAPHICS");
}
