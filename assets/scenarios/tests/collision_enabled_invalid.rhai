// Wrongly typed collision flags fault at projection rather than enabling a shape.
const TEST_KIND = "headless";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/World/Enabled")],entity_writes:[],query_reads:["ListOpenDocuments","QueryUsdPrim","QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.opened=false;this.started=false;
    expect_runtime_fault("usd-avian-collider-invalid");
}
fn on_tick(me,ctx) {
    if this.started {return;}
    let doc=();
    for document in query("ListOpenDocuments",#{}).open_documents {
        if document.origin!=() && document.origin.path.ends_with("collision_enabled_invalid.usda") {doc=document.doc_id;}
    }
    if doc==() {
        if !this.opened {this.opened=true;cmd("OpenFile",#{path:"lunco://scenes/tests/collision_enabled_invalid.usda"});}
        if sim_tick()>120 {throw("the collision flag fixture document did not open");}
        return;
    }
    if state_ready()!=true {throw("the enabled control was not projected before the invalid edit");}
    let source=query("QueryUsdPrim",#{doc_id:doc,path:"/World"});
    this.started=true;
    let result=cmd("ApplyUsdOps",#{doc_id:doc,parent_gen:source.generation,label:"Exercise invalid collision enablement",
        ops:[
            #{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"Malformed",type_name:"Cube",reference:(),reference_prim_path:()}},
            #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Malformed",name:"physics:collisionEnabled",type_name:"float",value:"1.0"}},
            #{SetApiSchemas:#{edit_target:"@runtime@",path:"/World/Malformed",schemas:["PhysicsCollisionAPI"]}}
        ]});
    if result.ok!=true {throw("invalid authored flag stimulus was rejected before projection");}
}
fn state_ready() {query("QueryPhysicsState",#{id:find_path("/World/Enabled")}).collider_present}
