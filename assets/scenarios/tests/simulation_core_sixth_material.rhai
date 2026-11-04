// @timing presentation
const GOOD="/World/SixthGoodSurface";
const BAD="/World/SixthBadSurface";
const GOOD_MATERIAL="/World/SixthGoodMaterial";
const BAD_MATERIAL="/World/SixthBadMaterial";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryPhysicsState"],required_inputs:[]}
}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn fixture(surface,material,terrain,friction) {
    let schemas=["PhysicsCollisionAPI","MaterialBindingAPI"];
    if terrain {schemas.push("LunCoTerrainAPI");}
    [#{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:material.split("/").pop(),type_name:"Material"}},
     #{SetApiSchemas:#{edit_target:"@runtime@",path:material,schemas:["PhysicsMaterialAPI"]}},
     attr(material,"physics:dynamicFriction","float",friction),
     #{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:surface.split("/").pop(),type_name:"Cube"}},
     #{SetApiSchemas:#{edit_target:"@runtime@",path:surface,schemas:schemas}},
     #{SetTranslate:#{edit_target:"@runtime@",path:surface,value:[50.0,0.0,0.0]}},
     #{SetRelationship:#{edit_target:"@runtime@",path:surface,name:"material:binding:physics",targets:[material]}}]
}
fn state(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn finish(state,error) {
    if state.done {return state;}
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE SIXTH MATERIAL "+state.mode,"SIMULATION_CORE_SIXTH_MATERIAL_"+state.mode);state
}
fn on_start(me,ctx) {
    this=#{doc:ctx.doc_id,mode:ctx.mode,frames:0,pending:(),done:false,checks:[],
        awaited:"initial scene readiness",target:""};
    if !["static","terrain"].contains(this.mode) {throw("material mode must be static or terrain");}
}
fn initial_scene_ready(me) {
    if this.done {return true;}
    let ready=query("GetReadiness");ready.ready && !ready.world_hold
}
fn receipt_ready(me) {
    this.done || this.pending==() || command_result(this.pending).status!="pending"
}
fn accept_material_command(me) {
    if this.done {return;}
    try {
        let result=command_result(this.pending);
        if !result.ok {throw("material fixture command rejected: "+result);}
        this.pending=();this.awaited="native material projection for "+this.target;
    } catch(error) {this=finish(this,error);}
}
fn author_valid_material(me) {
    if this.done {return;}
    try {
        this.target=GOOD;this.awaited="valid material command receipt";
        this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:fixture(GOOD,GOOD_MATERIAL,this.mode=="terrain","0.7")}).id;
    } catch(error) {this=finish(this,error);}
}
fn valid_surface_ready(me) {
    if this.done {return true;}
    try {
        let ready=query("GetReadiness");
        return ready.ready && !ready.world_hold && find_path(GOOD)>=0 && state(GOOD).collider_present;
    } catch(error) {this=finish(this,error);return true;}
}
fn check_valid_surface(me) {
    if this.done {return;}
    try {
        let ready=query("GetReadiness");
        this.checks.push(t_true(state(GOOD).collider_present,"valid bound material admits its native collision surface"));
        this.checks.push(t_true(!ready.faulted && ready.ready,"valid material retains healthy simulation admission"));
    } catch(error) {this=finish(this,error);}
}
fn author_invalid_material(me) {
    if this.done {return;}
    try {
        this.target=BAD;this.awaited="invalid material command receipt";
        this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:fixture(BAD,BAD_MATERIAL,this.mode=="terrain","-1")}).id;
    } catch(error) {this=finish(this,error);}
}
fn invalid_surface_ready(me) {
    if this.done {return true;}
    let ready=query("GetReadiness");
    ready.faulted && clock_snapshot().physics_paused && find_path(BAD)>=0
}
fn check_invalid_surface(me) {
    if this.done {return;}
    try {
        let ready=query("GetReadiness");
        this.checks.push(t_true(ready.fault.kind=="usd-avian-collider-invalid" && ready.fault.subject==BAD,
            "malformed material rejects the owning collision projection"));
        this.checks.push(t_true(ready.fault.detail.contains(BAD_MATERIAL) && ready.fault.detail.contains("physics:dynamicFriction")
            && ready.fault.detail.contains("-1"),"material rejection retains the decoder property, source path and authored value"));
        this.checks.push(t_true(ready.application_available && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),
            "invalid material holds physics without crashing"));
        this.checks.push(t_true(!state(BAD).collider_present,"rejected material does not admit an invalid native collider"));
        this.checks.push(t_true(state(GOOD).collider_present,"material rejection preserves the valid native control surface"));
    } catch(error) {this=finish(this,error);}
}
fn report_material(me) {if !this.done {this=finish(this,());}}
fn task(me,ctx) {
    seq([
        wait_until(Fn("initial_scene_ready")),
        once(Fn("author_valid_material")),
        wait_until(Fn("receipt_ready")),
        once(Fn("accept_material_command")),
        wait_until(Fn("valid_surface_ready")),
        once(Fn("check_valid_surface")),
        once(Fn("author_invalid_material")),
        wait_until(Fn("receipt_ready")),
        once(Fn("accept_material_command")),
        wait_until(Fn("invalid_surface_ready")),
        once(Fn("check_invalid_surface")),
        once(Fn("report_material"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;}this.frames+=1;
    if this.frames>1600 {this=finish(this,"material admission timed out awaiting "+this.awaited);}
}
