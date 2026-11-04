// @timing presentation
const BODY="/World/FifthAllocatorBody";
const ROOT=BODY+"/Allocator";
const MOUNT=ROOT+"/Mount";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","GetPort","QueryPhysicsState","GeneratedModelicaSource"],required_inputs:[]}
}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn add(parent,name,schemas) {[
    #{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:"Xform"}},
    #{SetApiSchemas:#{edit_target:"@runtime@",path:parent+"/"+name,schemas:schemas}}]}
fn append(ops,more) {for op in more {ops.push(op);}ops}
fn generated() {
    let rows=query("GeneratedModelicaSource",#{network_root:ROOT,include_source:true}).networks;
    if rows.len==0 {return ();}rows[0]
}
fn port(path,name) {query("GetPort",#{api_id:find_path(path),name:name}).value}
fn near(value,target) {value!=() && abs(value-target)<0.0001}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FIFTH ALLOCATOR","SIMULATION_CORE_FIFTH_ALLOCATOR");state
}
fn on_start(me,ctx) {
    this=#{doc:ctx.doc_id,phase:0,frames:0,done:false,checks:[],pending:(),tick:0,source:(),generation:()};
}
fn on_tick(me,ctx) {
    if this.done {return;}this.frames+=1;
    if this.frames>2400 {this.checks.push("live COM allocator timed out at phase "+this.phase);this=finish(this);return;}
    try {
        if this.pending!=() {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if !result.ok {throw("allocator fixture command rejected: "+result);}
            this.pending=();this.tick=clock_snapshot().sim_tick;
        }
        let ready=query("GetReadiness");
        if this.phase==0 {
            if !ready.ready || ready.world_hold {return;}
            let ops=add("/World","FifthAllocatorBody",["PhysicsRigidBodyAPI","PhysicsMassAPI"]);
            ops.push(attr(BODY,"physics:mass","float","2"));
            ops.push(attr(BODY,"physics:diagonalInertia","float3","(1,1,1)"));
            ops.push(attr(BODY,"physics:centerOfMass","point3f","(1,0,0)"));
            ops=append(ops,add(BODY,"Allocator",["LunCoDomainSynthesisAPI","CollectionAPI:components"]));
            ops.push(attr(ROOT,"lunco:synthesizer","token","\"actuator-wrench\""));
            ops.push(attr(ROOT,"collection:components:expansionRule","token","\"explicitOnly\""));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:ROOT,name:"collection:components:includes",targets:[MOUNT]}});
            for name in ["desired_force_x","desired_force_y","desired_force_z","desired_torque_x","desired_torque_y","desired_torque_z"] {
                ops.push(attr(ROOT,"inputs:"+name,"double",if name=="desired_force_y" {"10"} else {"0"}));
            }
            ops.push(attr(ROOT,"outputs:valve","double","0"));
            ops=append(ops,add(ROOT,"Mount",["LunCoForceActuatorAPI"]));
            ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:MOUNT,value:[1.0,0.0,0.0]}});
            ops.push(attr(MOUNT,"lunco:forceActuator:direction","float3","(0,1,0)"));
            ops.push(attr(MOUNT,"lunco:forceActuator:maxForce","double","10"));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:MOUNT,name:"lunco:forceActuator:commandSource",targets:[ROOT+".outputs:valve"]}});
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;
            cmd("SetTimeTransport",#{playing:true});this.phase=1;return;
        }
        let row=generated();
        if this.phase==5 {
            if row==() || row.projection_error==() || !row.projection_error.contains("same rigid body") {return;}
            this.checks.push(t_true(true,"mixed-body actuator collection rejects an ambiguous wrench reference"));
            this.checks.push(t_true(ready.application_available && query("QueryPhysicsState",#{id:find_path(BODY)}).computed_inertia_finite,
                "rejected mixed-body synthesis leaves the application and native body valid"));
            this=finish(this);return;
        }
        if !ready.ready || ready.world_hold || row==() || clock_snapshot().sim_tick-this.tick<8 {return;}
        if this.phase==1 {
            if !near(port(ROOT,"valve"),1.0) {return;}
            this.source=row.source;this.generation=row.document_generation;
            this.checks.push(t_true(row.input_bindings.body_com_x==BODY+".inputs:com_x"
                && row.input_bindings.body_com_y==BODY+".inputs:com_y"
                && row.input_bindings.body_com_z==BODY+".inputs:com_z","generated inputs bind the exact native body's three COM ports"));
            this.checks.push(t_true(row.source.contains("center_of_mass_x = body_com_x"),"generated Modelica consumes live COM inputs"));
            this.checks.push(t_true(near(port(BODY,"com_x"),1.0),"native authored COM is admitted"));
            this.checks.push(t_true(near(port(ROOT,"valve"),1.0),"COM at actuator mount produces force without a spurious torque penalty"));
            this.checks.push(t_true(query("QueryPhysicsState",#{id:find_path(BODY)}).computed_inertia_finite,"initial native mass properties remain finite"));
            this.pending=cmd("SetPorts",#{target:find_path(BODY),writes:[["com_x",0.0]]}).id;this.phase=2;return;
        }
        if this.phase==2 {
            if !near(port(ROOT,"valve"),0.5) {return;}
            this.checks.push(t_true(near(port(BODY,"com_x"),0.0),"live native COM input changes the authoritative wrench reference"));
            this.checks.push(t_true(near(port(ROOT,"valve"),0.5),"mixed translation and torque least squares uses the live COM"));
            this.checks.push(t_true(row.source==this.source && row.document_generation==this.generation,"COM changes do not regenerate or recompile source"));
            this.pending=cmd("SetPorts",#{target:find_path(ROOT),writes:[["desired_force_y",0.0],["desired_torque_z",10.0]]}).id;
            this.phase=3;return;
        }
        if this.phase==3 {
            if !near(port(ROOT,"valve"),0.5) {return;}
            this.checks.push(t_true(near(port(ROOT,"valve"),0.5),"mixed torque-only demand uses the same physical COM wrench"));
            this.pending=cmd("SetPorts",#{target:find_path(ROOT),writes:[["desired_force_y",100.0],["desired_torque_z",0.0]]}).id;
            this.phase=4;return;
        }
        if this.phase==4 {
            if !near(port(ROOT,"valve"),1.0) {return;}
            this.checks.push(t_true(near(port(ROOT,"valve"),1.0),"saturated demand retains the physical bounded command"));
            this.checks.push(t_true(row.source==this.source && row.document_generation==this.generation,"demand changes leave the generated compilation unit stable"));
            this.checks.push(t_true(near(port(ROOT,"body_com_x"),0.0),"the native COM source is consumed by the generated solver input"));
            let other="/World/FifthOtherBody";
            let mount=other+"/Mount";
            let ops=add("/World","FifthOtherBody",["PhysicsRigidBodyAPI","PhysicsMassAPI"]);
            ops.push(attr(other,"physics:mass","float","1"));
            ops.push(attr(other,"physics:diagonalInertia","float3","(1,1,1)"));
            ops=append(ops,add(other,"Mount",["LunCoForceActuatorAPI"]));
            ops.push(attr(mount,"lunco:forceActuator:direction","float3","(0,1,0)"));
            ops.push(attr(mount,"lunco:forceActuator:maxForce","double","10"));
            ops.push(attr(ROOT,"outputs:other","double","0"));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:mount,name:"lunco:forceActuator:commandSource",targets:[ROOT+".outputs:other"]}});
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:ROOT,name:"collection:components:includes",targets:[MOUNT,mount]}});
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=5;
        }
    } catch(error) {this.checks.push(error.to_string());this=finish(this);}
}
