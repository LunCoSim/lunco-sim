// @timing presentation
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryPhysicsState"],required_inputs:[]}
}
fn attr(path,name,value) {#{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:"float",value:value}}}
fn native(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FOURTH WHEEL","SIMULATION_CORE_FOURTH_WHEEL");state
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,pending:(),checks:[],recent:false,
        doc:ctx.doc_id,wheel:ctx.wheel_path,initial:ctx.initial_invalid==true};
}
fn on_event(me,event,ctx) {
    let expected=if this.initial {"usd-wheel-parameters-invalid"} else {"usd-wheel-resync-invalid"};
    let subject=if this.initial {"/PhysicalRoverControls/InitialInvalidWheel"} else {this.wheel};
    if event.name==expected && event.severity=="Error" && event.value.contains(subject)
        && event.value.contains("physxVehicleWheel:moi") {this.recent=true;}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>900 {this.checks.push("wheel admission timed out at phase "+this.phase);this=finish(this);return;}
    try {
        let ready=query("GetReadiness");
        if this.phase==0 {
            if !ready.ready || find_path(this.wheel)<0 {return;}
            let before=native(this.wheel);
            if before.mass_kg==() || !before.computed_inertia_finite {return;}
            this.before=before;
            this.checks.push(t_true(!ready.faulted && before.mass_kg>0.0,"physical wheel starts admitted with positive mass"));
            this.checks.push(t_true(before.computed_inertia_finite && before.inertia_tensor_kgm2!=(),"positive wheel has a readable complete native tensor"));
            let ops=[];
            if this.initial {
                ops=[#{AddPrim:#{edit_target:"@runtime@",parent_path:"/PhysicalRoverControls",name:"InitialInvalidWheel",type_name:"Cylinder",reference:()}},
                    #{SetApiSchemas:#{edit_target:"@runtime@",path:"/PhysicalRoverControls/InitialInvalidWheel",schemas:["PhysxVehicleWheelAPI"]}}];
            } else {ops=[attr(this.wheel,"physxVehicleWheel:moi","1e-40")];}
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;
            this.phase=1;return;
        }
        if this.phase==1 {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if !result.ok {throw("negative authored input did not reach its owning reader: "+result);}
            if !ready.faulted || !this.recent || !clock_snapshot().physics_paused {return;}
            let expected=if this.initial {"usd-wheel-parameters-invalid"} else {"usd-wheel-resync-invalid"};
            this.checks.push(t_true(result.ok,"typed authored edit reaches the projection owner"));
            this.checks.push(t_true(ready.fault.kind==expected && ready.application_available
                && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),"invalid wheel raises a structured fault and holds integration without crashing"));
            this.checks.push(t_true(this.recent,"Recent Events identifies the wheel and moi property"));
            let after=native(this.wheel);
            this.checks.push(t_true(after.mass_kg==this.before.mass_kg && after.computed_inertia_finite
                && after.inertia_tensor_kgm2==this.before.inertia_tensor_kgm2,"rejection preserves the valid native wheel candidate"));
            cmd("SetTimeTransport",#{playing:true});
            this.held_tick=clock_snapshot().sim_tick;this.held_frame=this.frames;this.phase=2;return;
        }
        if this.phase==2 && this.frames-this.held_frame>=10 {
            this.checks.push(t_true(clock_snapshot().sim_tick==this.held_tick && query("GetReadiness").application_available,
                "attempted resume remains held and the process stays available"));
            this=finish(this);
        }
    } catch(error) {this.checks.push(error.to_string());this=finish(this);}
}
