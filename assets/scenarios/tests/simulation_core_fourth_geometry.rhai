// @timing presentation
const MASS="/World/Target/AuditMass";
const ALLOCATOR="/World/ForceBody/FourthPrecision";
const MOUNT=ALLOCATOR+"/Mount";

fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","GetPort","QueryPhysicsState","QueryUsdPrim",
        "ListOpenDocuments","GetDocumentSource","CompileStatus"],required_inputs:[]}
}
fn attribute(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn add(parent,name,schemas) {[
    #{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:"Xform"}},
    #{SetApiSchemas:#{edit_target:"@runtime@",path:parent+"/"+name,schemas:schemas}}]}
fn center() {
    let body=find_path("/World/Target");
    let result=[];
    for name in ["com_x","com_y","com_z"] {
        let sample=query("GetPort",#{api_id:body,name:name});
        if sample.value==() {throw("folded COM is unavailable: "+sample);}
        result.push(sample.value);
    }
    result
}
fn near(actual,expected) {
    for axis in 0..3 {if abs(actual[axis]-expected[axis])>0.000001 {return false;}}
    true
}
fn finish(state) {
    state.done=true;
    let suffix=if state.negative {" RANGE"} else {""};
    report_verdict(state.checks,"SIMULATION CORE FOURTH GEOMETRY"+suffix,"SIMULATION_CORE_FOURTH_GEOMETRY");state
}
fn on_start(me,ctx) {
    this=#{done:false,phase:0,elapsed:0.0,checks:[],pending:(),doc:ctx.doc_id,tick:0,
        negative:ctx.native_invalid==true,recent:false,frames:0};
}
fn on_event(me,event,ctx) {
    if event.name=="cosim-force-actuator-range" && event.severity=="Error"
        && event.value.contains("offset_m") && event.value.contains("local_position_m") {
        this.recent=true;
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    try {
        this.elapsed+=execution_context().delta_seconds;
        this.frames+=1;
        if this.elapsed>90.0 {throw("geometry timed out at phase "+this.phase);}
        let ready=query("GetReadiness");
        if this.phase==5 {
            if !ready.faulted || !this.recent || !clock_snapshot().physics_paused {return;}
            this.checks.push(t_true(ready.fault.kind=="cosim-force-actuator-range"
                && ready.application_available && clock_snapshot().physics_paused,
                "out-of-range native moment holds physics without crashing: "+ready));
            this.checks.push(t_true(this.recent,"Recent Events identifies the force and lever-arm conversion"));
            this.checks.push(t_true(query("QueryPhysicsState",#{id:find_path("/World/ForceBody")}).computed_inertia_finite,
                "rejected point force leaves the native body finite"));
            cmd("SetTimeTransport",#{playing:true});
            this.tick=clock_snapshot().sim_tick;this.held_frame=this.frames;this.phase=6;return;
        }
        if this.phase==6 {
            if this.frames-this.held_frame<10 {return;}
            this.checks.push(t_true(clock_snapshot().sim_tick==this.tick && ready.application_available,
                "native range failure remains held after attempted resume"));
            this=finish(this);return;
        }
        if ready.faulted {throw("geometry fixture faulted: "+ready);}
        if this.pending!=() {
            let receipt=command_result(this.pending);
            if receipt.status=="pending" {return;}
            if receipt.ok!=true {throw("geometry edit rejected: "+receipt);}
            this.pending=();this.tick=clock_snapshot().sim_tick;
        }
        if !ready.ready || ready.world_hold {return;}
        if this.phase==0 {
            if !near(center(),[0.0,1.0/30.0,0.0]) {return;}
            this.checks.push(t_true(true,"baseline reduced COM is admitted"));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                #{SetScale:#{edit_target:"@runtime@",path:MASS,value:[2.0,2.0,2.0]}}
            ]}).id;this.phase=1;return;
        }
        if clock_snapshot().sim_tick-this.tick<3 {return;}
        if this.phase==7 {
            if clock_snapshot().sim_tick-this.tick<6 {return;}
            let body=query("QueryPhysicsState",#{id:find_path("/World/ForceBody")});
            this.checks.push(t_true(abs(body.angular_velocity_radps[2])>0.01 && body.computed_inertia_finite,
                "representable point force supplies native torque through the double mount"));
            this.checks.push(t_true(ready.ready && !ready.faulted,"valid native point force remains admitted"));
            cmd("SetPorts",#{target:find_path(MOUNT),writes:[["force_command",0.0]]});
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                attribute(MOUNT,"lunco:forceActuator:maxForce","double","1e31")]}).id;
            this.phase=4;return;
        }
        if this.phase==4 {
            if get(find_path(MOUNT),"ForceActuator.max_force_n")!=1.0e31 {return;}
            this.pending=cmd("SetPorts",#{target:find_path(MOUNT),writes:[["force_command",1.0e31]]}).id;
            this.phase=5;return;
        }
        if this.phase==1 {
            this.checks.push(t_true(near(center(),[0.0,1.0/15.0,0.0]),
                "uniform scale updates the folded authored COM: "+center()));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                #{SetScale:#{edit_target:"@runtime@",path:MASS,value:[2.0,3.0,4.0]}},
                #{SetRotate:#{edit_target:"@runtime@",path:MASS,value:[0.0,0.0,45.0]}}
            ]}).id;this.phase=2;return;
        }
        if this.phase==2 {
            let expected=0.4/sqrt(2.0)/6.0;
            this.checks.push(t_true(near(center(),[expected,expected,0.0]),
                "nonuniform scale and rotation update COM in the owning body frame: "+center()));
            this.checks.push(t_true(query("QueryPhysicsState",#{id:find_path("/World/Target")}).computed_inertia_finite,
                "scaled contribution retains finite native inertia"));
            let ops=add("/World/ForceBody","FourthPrecision",["LunCoDomainSynthesisAPI","CollectionAPI:components"]);
            ops.push(attribute(ALLOCATOR,"lunco:synthesizer","token","\"actuator-wrench\""));
            ops.push(attribute(ALLOCATOR,"collection:components:expansionRule","token","\"explicitOnly\""));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:ALLOCATOR,name:"collection:components:includes",targets:[MOUNT]}});
            ops.push(attribute(ALLOCATOR,"outputs:valve","double","0"));
            for op in add(ALLOCATOR,"Mount",["LunCoForceActuatorAPI"]) {ops.push(op);}
            ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:MOUNT,value:[100000001.0,0.0,0.0]}});
            ops.push(attribute(MOUNT,"lunco:forceActuator:direction","float3","(0, 1, 0)"));
            ops.push(attribute(MOUNT,"lunco:forceActuator:maxForce","double","10"));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:MOUNT,name:"lunco:forceActuator:commandSource",targets:[ALLOCATOR+".outputs:valve"]}});
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=3;return;
        }
        let compiled_source=();
        for doc in query("ListOpenDocuments").open_documents {
            if doc.kind!="modelica" {continue;}
            let source=query("GetDocumentSource",#{doc_id:doc.doc_id});
            if source.source.contains("FourthPrecision_System") && query("CompileStatus",#{doc_id:doc.doc_id}).is_compiled {
                compiled_source=source.source;break;
            }
        }
        if compiled_source==() {return;}
        let lever=vec3_array(get(find_path(MOUNT),"ForceActuator.local_position"));
        this.checks.push(t_true(lever[0]==100000001.0,"actuator mount retains its authored double precision"));
        this.checks.push(t_true(compiled_source.contains("wrench_matrix = [0.0; 10.0; 0.0; 0.0; 0.0; 1000000010.0]"),
            "compiled allocator source retains the exact double-precision torque coefficient"));
        this.checks.push(t_true(query("QueryUsdPrim",#{path:MOUNT}).attrs["xformOp:translate"][0]==100000001.0,
            "composed USD retains the exact authored double translation"));
        this.checks.push(t_true(ready.ready && !ready.world_hold && !ready.faulted,
            "canonical geometry and generated solver remain admitted"));
        if this.negative {
            this.pending=cmd("SetPorts",#{target:find_path(MOUNT),writes:[["force_command",1.0e-8]]}).id;
            this.phase=7;return;
        }
        this=finish(this);
    } catch(error) {this.checks.push("fourth geometry failed: "+error);this=finish(this);}
}
