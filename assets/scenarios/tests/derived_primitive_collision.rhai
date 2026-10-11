// A live edit to a visual source must reject a stale admitted primitive proxy.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/World/Body")],entity_writes:[],query_reads:["ListOpenDocuments","QueryUsdPrim","QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.doc=();this.started=false;this.opened=false;
    expect_runtime_fault("usd-avian-collider-invalid");
}
fn on_tick(me,ctx) {
    if this.started {return;}
    let documents=query("ListOpenDocuments",#{});
    for document in documents.open_documents {
        if document.origin!=() && document.origin.path.ends_with("derived_primitive_collision.usda") {this.doc=document.doc_id;}
    }
    if this.doc==() {
        if !this.opened {this.opened=true;cmd("OpenFile",#{path:"lunco://scenes/tests/derived_primitive_collision.usda"});}
        if sim_tick()>120 {throw("the exact source document did not open");}
        return;
    }
    let proxy=query("QueryUsdPrim",#{doc_id:this.doc,path:"/World/Body/Proxy",collision_geometry:true});
    let body=query("QueryPhysicsState",#{id:find_path("/World/Body")});
    if proxy==() || proxy.collision_geometry==() || body.physics_state_ready!=true
        || body.collider_source_paths!=["/World/Body/Proxy"] {throw("valid derived primitive was not admitted before the negative edit");}
    this.started=true;
    let source=query("QueryUsdPrim",#{doc_id:this.doc,path:"/World/Body/Visual"});
    let result=cmd("ApplyUsdOps",#{doc_id:this.doc,parent_gen:source.generation,label:"Exercise stale primitive rejection",
        ops:[#{SetAttribute:#{edit_target:"@runtime@",path:"/World/Body/Visual",name:"size",type_name:"double",value:"2.0"}}]});
    if result.ok!=true {throw("stale-source stimulus was rejected before projection");}
}
