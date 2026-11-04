// @timing presentation
// Live document edits must refresh physical metadata before simulation admission.
const SOURCE="/World/Source";
const MOUNT=SOURCE+"/AuditMount";
const CARRIER=SOURCE+"/AuditCarrier";
const NESTED=CARRIER+"/NestedMount";
const MASS="/World/Target/AuditMass";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","RuntimeDiagnostics","InspectUsdDocument","GetPort","QueryUsdPrim"],required_inputs:[]}
}
fn translate(path,value) {#{SetTranslate:#{edit_target:"@runtime@",path:path,value:value}}}
fn attribute(path,name,type_name,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:type_name,value:value}}
}
fn add(parent,name) {#{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:"Xform"}}}
fn apply(doc,ops) {
    let receipt=cmd("ApplyUsdOps",#{doc_id:doc,ops:ops});
    if receipt.id==() {throw("mount edit has no command receipt: "+receipt);}
    receipt.id
}
fn vector(path,field) {
    let entity=find_path(path);
    if entity<0 {return ();}
    let value=get(entity,field);
    if value==() {return ();}
    vec3_array(value)
}
fn near(value,expected) {
    if value==() {return false;}
    for axis in 0..3 {if abs(value[axis]-expected[axis])>0.00001 {return false;}}
    true
}
fn port(path,name) {
    let entity=find_path(path);if entity<0 {return ();}
    let result=query("GetPort",#{api_id:entity,name:name});
    if result==() {return ();}result.value
}
fn lever(path) {vector(path,"ForceActuator.local_position")}
fn direction(path) {vector(path,"ForceActuator.direction_local")}
fn torque() {vector(MOUNT,"TorqueActuator.axis_local")}
fn on_start(me,ctx) {
    this.frames=0;this.done=false;this.phase=0;this.pending=();this.checks=[];this.doc=ctx.doc_id;
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1200 {
        this.done=true;this.checks.push("physical mount scenario timed out at phase "+this.phase
            +"; source position_x="+port(SOURCE,"position_x"));
        report_verdict(this.checks,"SIMULATION CORE THIRD MOUNTS","SIMULATION_CORE_THIRD_MOUNTS");return;
    }
    try {
        if this.doc==() {throw("mount scenario requires the supplied fixture doc_id");}
        let ready=query("GetReadiness");
        if ready.faulted {throw("physical mount fixture faulted: "+ready);}
        if this.pending!=() {
            let completed=command_result(this.pending);
            if completed.status=="pending" {return;}
            if completed.ok!=true {throw("mount edit rejected: "+completed);}
            this.pending=();
        }
        if this.phase==0 {
            if !ready.ready || !near(lever(MOUNT),[1.0,0.0,0.0]) {return;}
            this.checks.push(t_true(near(lever(MOUNT),[1.0,0.0,0.0]),"initial actuator lever"));
            this.checks.push(t_true(near(direction(MOUNT),[0.0,1.0,0.0]),"initial actuator direction"));
            this.pending=apply(this.doc,[translate(MOUNT,[2.0,0.0,0.0])]);this.phase=1;return;
        }
        if this.phase==1 {
            if !near(lever(MOUNT),[2.0,0.0,0.0]) {return;}
            this.checks.push(t_true(near(lever(MOUNT),[2.0,0.0,0.0]) && near(direction(MOUNT),[0.0,1.0,0.0]),"live translation refreshes the lever only"));
            this.pending=apply(this.doc,[
                #{SetApiSchemas:#{edit_target:"@runtime@",path:MOUNT,schemas:["LunCoForceActuatorAPI","LunCoTorqueActuatorAPI"]}},
                attribute(MOUNT,"lunco:torqueActuator:axis","float3","(0, 1, 0)"),
                attribute(MOUNT,"lunco:torqueActuator:maxTorque","double","10")]);
            this.phase=2;return;
        }
        if this.phase==2 {
            if !near(torque(),[0.0,1.0,0.0]) {return;}
            this.checks.push(t_true(near(torque(),[0.0,1.0,0.0]),"initial torque axis uses the body frame"));
            this.pending=apply(this.doc,[#{SetRotate:#{edit_target:"@runtime@",path:MOUNT,value:[0.0,0.0,90.0]}}]);
            this.phase=3;return;
        }
        if this.phase==3 {
            if !near(direction(MOUNT),[-1.0,0.0,0.0]) || !near(torque(),[-1.0,0.0,0.0]) {return;}
            this.checks.push(t_true(near(direction(MOUNT),[-1.0,0.0,0.0]),"live mount rotation refreshes force direction"));
            this.checks.push(t_true(near(torque(),[-1.0,0.0,0.0]),"live mount rotation refreshes torque axis"));
            this.checks.push(t_true(near(lever(MOUNT),[2.0,0.0,0.0]),"mount rotation preserves the lever"));
            this.pending=apply(this.doc,[translate(SOURCE,[100000010.0,2.0,0.0])]);this.phase=4;return;
        }
        if this.phase==4 {
            let position=port(SOURCE,"position_x");if position==() || abs(position-100000010.0)>0.00001 {return;}
            this.checks.push(t_true(near(lever(MOUNT),[2.0,0.0,0.0]) && near(direction(MOUNT),[-1.0,0.0,0.0]) && near(torque(),[-1.0,0.0,0.0]),"moving the entire body preserves relative actuator metadata"));
            this.pending=apply(this.doc,[add(SOURCE,"AuditCarrier"),translate(CARRIER,[3.0,0.0,0.0]),
                add(CARRIER,"NestedMount"),translate(NESTED,[1.0,0.0,0.0]),
                #{SetApiSchemas:#{edit_target:"@runtime@",path:NESTED,schemas:["LunCoForceActuatorAPI"]}},
                attribute(NESTED,"lunco:forceActuator:direction","float3","(0, 1, 0)"),
                attribute(NESTED,"lunco:forceActuator:maxForce","double","10")]);this.phase=5;return;
        }
        if this.phase==5 {
            if !near(lever(NESTED),[4.0,0.0,0.0]) {return;}
            this.checks.push(t_true(near(lever(NESTED),[4.0,0.0,0.0]),"nested mount composes the carrier translation"));
            this.checks.push(t_true(near(direction(NESTED),[0.0,1.0,0.0]),"nested actuator direction"));
            this.pending=apply(this.doc,[translate(CARRIER,[5.0,0.0,0.0])]);this.phase=6;return;
        }
        if this.phase==6 {
            if !near(lever(NESTED),[6.0,0.0,0.0]) {return;}
            this.checks.push(t_true(near(lever(NESTED),[6.0,0.0,0.0]),"ancestor-only edit refreshes descendant mount"));
            this.checks.push(t_true(near(lever(MOUNT),[2.0,0.0,0.0]),"ancestor edit preserves its unaffected sibling"));
            this.pending=apply(this.doc,[translate(MASS,[0.0,1.0,0.0])]);this.phase=7;return;
        }
        if this.phase==7 {
            let center=port("/World/Target","com_y");
            if !near(vector(MASS,"RaycastMassContribution.local.translation"),[0.0,1.0,0.0]) || center==() || abs(center-0.2)>0.00001 {return;}
            this.checks.push(t_true(near(vector(MASS,"RaycastMassContribution.local.translation"),[0.0,1.0,0.0]),"live reduced mass mount refreshes"));
            this.checks.push(t_true(abs(center-0.2)<0.00001,"refreshed reduced mount reaches the native mass fold"));
            let generation=query("InspectUsdDocument",#{doc_id:this.doc}).generation;
            let rejected=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[translate(MOUNT,[9.0,0.0,0.0]),translate(NESTED,[0.0,"malformed",0.0])]});
            this.checks.push(t_true(rejected.ok==false,"malformed transform batch rejects"));
            this.checks.push(t_true(query("InspectUsdDocument",#{doc_id:this.doc}).generation==generation,"rejected transform batch preserves document generation"));
            this.checks.push(t_true(near(lever(MOUNT),[2.0,0.0,0.0]),"rejected batch preserves original lever"));
            this.checks.push(t_true(near(lever(NESTED),[6.0,0.0,0.0]),"rejected batch preserves nested lever"));
            this.checks.push(t_true(near(torque(),[-1.0,0.0,0.0]),"rejected batch preserves torque axis"));
            this.checks.push(t_true(!query("GetReadiness").faulted && query("RuntimeDiagnostics").errors==0,"rejected transform leaves the simulation healthy"));
            this.done=true;report_verdict(this.checks,"SIMULATION CORE THIRD MOUNTS","SIMULATION_CORE_THIRD_MOUNTS");
        }
    } catch(error) {
        this.done=true;this.checks.push(error.to_string());
        report_verdict(this.checks,"SIMULATION CORE THIRD MOUNTS","SIMULATION_CORE_THIRD_MOUNTS");
    }
}
