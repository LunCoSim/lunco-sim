// @timing presentation
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","InspectUsdDocument"],required_inputs:[]}
}
fn attr(name,type_name,value) {#{SetAttribute:#{edit_target:"@runtime@",path:"/World/Source/AuditMount",name:name,type_name:type_name,value:value}}}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,recent_error:false,negative:ctx.negative,doc:ctx.doc_id,checks:[]};
}
fn on_event(me,event,ctx) {
    if event.name=="usd-physical-mount-invalid" {
        this.recent_error=event.severity=="Error" && event.value.contains("AuditMount")
            && event.value.contains("invalid physical mount");
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    try {
        if this.frames>900 {throw("mount boundary timed out at phase "+this.phase);}
        if this.phase==0 {
            let ops=if this.negative { [attr("lunco:forceActuator:direction","float3","(0, 0, 0)")] }
            else { [#{SetApiSchemas:#{edit_target:"@runtime@",path:"/World/Source/AuditMount",schemas:["LunCoForceActuatorAPI","LunCoTorqueActuatorAPI"]}},
                attr("lunco:torqueActuator:axis","float3","(0, 1, 0)"),attr("lunco:torqueActuator:maxTorque","double","10")] };
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=1;return;
        }
        let result=command_result(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {throw("mount boundary edit failed before projection: "+result);}
        let ready=query("GetReadiness");
        if this.negative {
            let clock=clock_snapshot();if !ready.faulted || !clock.physics_paused || !this.recent_error {return;}
            this.checks.push(t_true(clock.physics_hold_reasons.contains("runtime-safety-failure"),"invalid authored actuator stops physics"));
            this.checks.push(t_true(ready.fault.kind=="usd-physical-mount-invalid","fault identifies the owning physical mount boundary"));
            this.checks.push(t_true(ready.fault.to_string().contains("AuditMount"),"fault identifies the authored prim"));
            this.checks.push(t_true(ready.application_available,"authored errors preserve the application and API"));
            this.checks.push(t_true(this.recent_error,"Recent Events identifies the invalid authored mount with error severity"));
        } else {
            let axis=get(find_path("/World/Source/AuditMount"),"TorqueActuator.axis_local");if axis==() || !ready.ready {return;}
            let v=vec3_array(axis);
            this.checks.push(t_true(abs(v[0])<0.00001 && abs(v[1])<0.00001 && abs(v[2]+1.0)<0.00001,"Z-up torque axis is canonical -Z"));
            let force=vec3_array(get(find_path("/World/Source/AuditMount"),"ForceActuator.direction_local"));
            this.checks.push(t_true(abs(force[2]-v[2])<0.00001,"force and torque share the composed frame reader"));
            this.checks.push(t_true(ready.ready && !ready.faulted,"valid converted mount remains admitted"));
        }
        this.done=true;
        report_verdict(this.checks,"SIMULATION CORE MOUNT BOUNDARY","SIMULATION_CORE_MOUNT_BOUNDARY");
    } catch(error) {
        this.done=true;report_verdict(["mount boundary failed: "+error],"SIMULATION CORE MOUNT BOUNDARY","SIMULATION_CORE_MOUNT_BOUNDARY");
    }
}
