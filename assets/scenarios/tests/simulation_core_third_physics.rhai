// @timing presentation
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryUsdPrim","InspectUsdDocument"],required_inputs:[]}
}
fn near(value,expected) {abs(value-expected)<0.00001}
fn local_mount() {vec3_array(get(find_path("/World/Vehicle/Wheel"),"WheelBodyMount.local.translation"))}
fn joint_ready(path) {
    let binding=query("QueryUsdPrim",#{path:path,topology:true}).topology.binding;
    binding.physics_joint_link!=() && !binding.physics_joint_pending
}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE THIRD PHYSICS","SIMULATION_CORE_THIRD_PHYSICS");state
}
fn on_start(me,ctx) {this=#{done:false,frames:0,phase:0,checks:[],recent_error:false,pending:(),doc:ctx.doc_id,invalid_slip:ctx.invalid_slip==true};}
fn on_event(me,event,ctx) {
    if event.name=="usd-wheel-resync-invalid" {
        this.recent_error=event.severity=="Error" && event.value.contains("/World/Vehicle/Wheel")
            && event.value.contains(if this.invalid_slip {"minActiveLongitudinalSlipDenominator"} else {"radius"});
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>900 {this.checks.push("third physics timed out at phase "+this.phase);this=finish(this);return;}
    try {
        let wheel=find_path("/World/Vehicle/Wheel");
        if wheel<0 {return;}
        let ready=query("GetReadiness");
        if this.phase==0 {
            if !ready.ready || clock_snapshot().sim_tick<30 {return;}
            if get(wheel,"WheelRaycast.wheel_radius")==() {return;}
            this.checks.push(t_true(!ready.faulted && !ready.world_hold,"centimetre stage is admitted"));
            this.checks.push(t_true(joint_ready("/World/SocketJoint"),"descendant joint is natively linked"));
            let position=query("QueryUsdPrim",#{path:"/World/Follower"}).world_position;
            this.checks.push(t_true(near(position[0],2.0) && near(position[1],41.0) && near(position[2],0.0),
                "scaled socket and explicit endpoint anchor retain the intended seated position: "+position));
            this.checks.push(t_true(joint_ready("/World/WorldJoint"),"empty world endpoint accepts standard anchor defaults"));
            this.checks.push(t_true(joint_ready("/World/DefaultJoint"),"independent omitted anchors admit a native joint"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.wheel_radius"),0.35),"wheel radius is converted to metres"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.wheel_width"),0.2),"wheel width is converted to metres"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.mass"),25.0),"wheel mass retains admitted kilograms"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.moment_of_inertia"),1.53125),"wheel inertia scales by squared length units"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.bearing_damping"),0.5),"rotational damping scales by squared length units"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.brake_torque_max"),120.0),"brake torque scales by squared length units"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.slip_stiffness"),14000.0),"longitudinal stiffness uses force units"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.lateral_stiffness_graph.max_stiffness"),10000.0)
                && near(get(wheel,"WheelRaycast.lateral_stiffness_graph.rest_load"),400.0),"lateral stiffness and rest load use force units"));
            this.checks.push(t_true(near(get(wheel,"Suspension.travel_distance"),0.15)
                && near(get(wheel,"Suspension.rest_length"),0.5),"standard travel converts and custom SI rest length does not"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.min_validated_speed"),0.2)
                && near(get(wheel,"Suspension.spring_k"),15000.0)
                && near(get(wheel,"Suspension.damping_c"),3000.0),"SI speed and length-independent spring coefficients are unchanged"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.active_mps"),0.2),
                "standard active slip speed converts centimetres per second to SI"));
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.passive_mps"),7.0),
                "standard passive slip speed converts independently of tire validity speed"));
            this.checks.push(t_true(near(local_mount()[0],1.0),"initial wheel mount uses body-local metres"));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[#{SetTranslate:#{
                edit_target:"@runtime@",path:"/World/Vehicle/Wheel",value:[2.0,0.0,0.0]}}]}).id;
            this.phase=1;return;
        }
        if this.phase==1 {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("wheel mount edit failed: "+result);}
            if !near(local_mount()[0],2.0) || !ready.ready {return;}
            this.checks.push(t_true(near(local_mount()[0],2.0),"live wheel edit refreshes its physical mount"));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[
                #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Vehicle",name:"physxVehicle:minActiveLongitudinalSlipDenominator",type_name:"float",value:"50"}},
                #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Vehicle",name:"physxVehicle:minPassiveLongitudinalSlipDenominator",type_name:"float",value:"0"}}]}).id;
            this.phase=2;return;
        }
        if this.phase==2 {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("vehicle slip parameter edit failed: "+result);}
            if !ready.ready || !near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.active_mps"),0.5)
                || !near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.passive_mps"),4.0) {return;}
            this.checks.push(t_true(near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.active_mps"),0.5)
                && near(get(wheel,"WheelRaycast.longitudinal_slip_denominators.passive_mps"),4.0),
                "live vehicle parameter edits refresh wheels and zero resolves the standard SI default once"));
            let invalid=if this.invalid_slip {
                #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Vehicle",name:"physxVehicle:minActiveLongitudinalSlipDenominator",type_name:"float",value:"-1"}}
            } else {
                #{SetAttribute:#{edit_target:"@runtime@",path:"/World/Vehicle/Wheel",name:"physxVehicleWheel:radius",type_name:"float",value:"0.0"}}
            };
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[invalid]}).id;
            this.phase=3;return;
        }
        if this.phase==3 {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("negative authored edit did not reach its projection owner: "+result);}
            let clock=clock_snapshot();if !ready.faulted || !clock.physics_paused || !this.recent_error {return;}
            this.checks.push(t_true(clock.physics_hold_reasons.contains("runtime-safety-failure") && ready.application_available
                && ready.fault.kind=="usd-wheel-resync-invalid",
                "invalid authored wheel parameters hold physics without crashing the application"));
            this.checks.push(t_true(this.recent_error,"Recent Events identifies the wheel and invalid parameter with error severity"));
            this=finish(this);
        }
    } catch(error) {this.checks.push("third physics failed: "+error);this=finish(this);}
}
