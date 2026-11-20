// @timing presentation
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryPhysicsState","CosimStatus","QueryTelemetryHistory"],required_inputs:[]}
}
fn require(result) {if result.ok!=true {throw(result.to_string());}}
fn settled() {
    let clock=clock_snapshot();
    !clock.barrier_held && !clock.simulation_progress.held && !clock.simulation_cycle_active
        && clock.queued_simulation_steps.to_int()==0
}
fn state(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn modelica_clock(path) {
    for model in query("CosimStatus",#{include_values:false}).entities {
        if model.name==path && (model.status=="Idle" || model.status=="Running") {
            return [model.modelica_current_time,model.modelica_target_time,
                model.modelica_next_communication_time,model.modelica_is_stepping];
        }
    }
    ()
}
fn finite_state(value) {
    for vector in [value.linear_velocity_mps,value.angular_velocity_radps] {
        for v in vector {if !v.is_finite() {return false;}}
    }
    value.computed_mass_finite && value.computed_inertia_finite
}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn finish(current,error) {
    if error!=() {current.checks.push(error.to_string());}
    current.done=true;
    report_verdict(current.checks,"SIMULATION CORE SIXTH NUMERIC "+current.mode,
        "SIMULATION_CORE_SIXTH_NUMERIC_"+current.mode);current
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,mode:ctx.mode,body:ctx.body_path,control:ctx.control_path,
        checks:[],recent:false,pending:(),pending_control:(),doc:ctx.doc_id,torque:ctx.torque,
        model:ctx.model_path};
    require(cmd("StepSimulation",#{hold:true}));
}
fn on_event(me,event,ctx) {
    if event.name=="physics-native-integration-range" && event.severity=="Error"
        && event.value.contains(this.body) && event.value.contains("linear_increment=") {
        this.recent=true;
    }
}
fn warm_up(me) {
    if this.mode!="overflow" {return;}
    let body=find_path(this.body);
    this.mass_key="api/"+body+":native_rejection_mass";
    this.position_key="api/"+body+":position.x";
    require(cmd("ControlTelemetry",#{rate_hz:60.0}));
    require(cmd("ControlTelemetry",#{entity:body,channel:"native_rejection_mass",port:"mass",
        unit:"kg",rate_hz:60.0}));
    require(cmd("StepSimulation",#{steps:1}));
}
fn stimulate(me) {
            this.before=state(this.body);
            this.checks.push(t_true(finite_state(this.before) && this.before.body_mode=="dynamic",
                "numeric fixture begins with finite admitted native state"));
            if this.mode!="inertia" {
                require(cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                    #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Physics",name:"physics:gravityMagnitude",
                        type_name:"float",value:if this.mode=="overflow" {"3e38"} else {"10"}}},
                    #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Physics",name:"physics:gravityDirection",
                        type_name:"vector3f",value:"(1, 0, 0)"}}
                ]}));
            }
            let writes=if this.mode=="inertia" {[["torque_x",this.torque]]}
                else {[["force_x",if this.mode=="overflow" {3e38} else {10.0}]]};
            this.pending=cmd("SetPorts",#{target:find_path(this.body),writes:writes});
            if this.mode=="inertia" {
                this.pending_control=cmd("SetPorts",#{target:find_path(this.control),writes:writes});
            }

}
fn admit_steps(me) {
            require(terminal(this.pending));
            if this.pending_control!=() {require(terminal(this.pending_control));}
            this.checks.push(t_true(true,"finite scalar effort commands receive terminal admission"));
            this.clock=clock_snapshot();
            this.start=state(this.body);
            if this.mode=="overflow" {
                this.model_clock=modelica_clock(this.model);
                this.mass_history=query("QueryTelemetryHistory",#{key:this.mass_key}).samples;
                this.position_history=query("QueryTelemetryHistory",#{key:this.position_key}).samples;
            }
            require(cmd("StepSimulation",#{steps:8}));

}
fn check_result(me) {
            let ready=query("GetReadiness");let sample=state(this.body);
            this.checks.push(t_true(finite_state(sample),"native body state remains finite and queryable"));
            if this.mode=="overflow" {
                let clock=clock_snapshot();
                this.checks.push(t_true(ready.faulted && ready.fault.kind=="physics-native-integration-range",
                    "combined persistent acceleration is rejected at native integration admission"));
                this.checks.push(t_true(ready.faulted && ready.fault.subject==this.body
                    && ready.fault.detail.contains("linear_increment="),
                    "numeric fault identifies the body and actual native increment"));
                this.checks.push(t_true(clock.physics_paused
                    && clock.physics_hold_reasons.contains("runtime-safety-failure"),
                    "invalid native increments hold integration"));
                this.checks.push(t_true(sample.linear_velocity_mps==this.start.linear_velocity_mps,
                    "rejected native increments never reach canonical body velocity"));
                this.checks.push(t_true(clock.clock_contract_ok && !clock.simulation_cycle_active
                    && clock.sim_tick==this.clock.sim_tick
                    && clock.fixed_elapsed_s==this.clock.fixed_elapsed_s
                    && clock.physics_elapsed_s==this.clock.physics_elapsed_s,
                    "native rejection preserves the master tick and both completed clocks"));
                this.checks.push(t_true(clock.world_sim_s==this.clock.world_sim_s
                    && clock.world_met_s==this.clock.world_met_s,
                    "world and mission time retain their last completed interval"));
                this.checks.push(t_true(this.model_clock[0]>0.0
                    && this.model_clock[0]==this.model_clock[1] && !this.model_clock[3]
                    && modelica_clock(this.model)==this.model_clock,
                    "the due Modelica participant keeps its committed and requested endpoints"));
                this.checks.push(t_true(!this.mass_history.is_empty() && !this.position_history.is_empty()
                    && query("QueryTelemetryHistory",#{key:this.mass_key}).samples==this.mass_history
                    && query("QueryTelemetryHistory",#{key:this.position_key}).samples==this.position_history,
                    "parameter and native position graphs retain exactly the accepted samples"));
            } else if this.mode=="inertia" {
                let control=state(this.control);
                this.checks.push(t_true(finite_state(control) && control.angular_velocity_radps[0]>0.05,
                    "isotropic control remains finite and responds to the torque"));
                this.checks.push(t_true(sample.angular_velocity_radps[0]>0.05,
                    "valid tiny anisotropic inertia responds to finite torque"));
                this.checks.push(t_true(abs(sample.angular_velocity_radps[0]-control.angular_velocity_radps[0])<0.00001,
                    "anisotropic and isotropic rotors retain equal principal-axis acceleration"));
                this.checks.push(t_true(ready.ready && !ready.world_hold && !ready.faulted,
                    "both supported tiny tensors remain admitted"));
            } else {
                let elapsed=clock_snapshot().physics_elapsed_s-this.clock.physics_elapsed_s;
                this.checks.push(t_true(abs(sample.linear_velocity_mps[0]-this.start.linear_velocity_mps[0]-20.0*elapsed)<0.0001,
                    "finite force and persistent gravity both contribute at the native interval"));
                this.checks.push(t_true(ready.ready && !ready.world_hold && !ready.faulted,
                    "finite combined native acceleration remains admitted"));
            }

}
fn report(me) {
            if this.mode=="overflow" {this.checks.push(t_true(this.recent,"Recent Events contains the numeric admission Error"));}
            this=finish(this,());

}
fn task(me,ctx) {
    seq([
        wait_until(|m| settled() && query("GetReadiness").ready
            && (this.mode!="overflow" || modelica_clock(this.model)!=())),
        once(Fn("warm_up")),
        wait_until(|m| settled()),
        once(Fn("stimulate")),
        wait_until(|m| terminal(this.pending).status!="pending"
            && (this.pending_control==() || terminal(this.pending_control).status!="pending")),
        once(Fn("admit_steps")),
        wait_until(|m| query("GetReadiness").faulted || settled()),
        wait(0.1),
        once(Fn("check_result")),
        wait_until(|m| this.mode!="overflow" || this.recent),
        once(Fn("report"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;} this.frames+=1;
    if this.frames>2400 {this=finish(this,"numeric gate timed out: "+query("GetReadiness"));}
}
