// @timing presentation
const PREFIX="/World/SixthHinge";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryUsdPrim","GetPort","QueryPhysicsState"],required_inputs:[]}
}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn append(ops,more) {for op in more {ops.push(op);}ops}
fn body(name,rotation,kinematic) {
    let path=PREFIX+name;
    let ops=[#{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"SixthHinge"+name,type_name:"Xform"}},
      #{SetApiSchemas:#{edit_target:"@runtime@",path:path,schemas:["PhysicsRigidBodyAPI","PhysicsMassAPI"]}},
      #{SetTranslate:#{edit_target:"@runtime@",path:path,value:[100.0,0.0,0.0]}},
      #{SetRotate:#{edit_target:"@runtime@",path:path,value:rotation}},
      attr(path,"physics:mass","float","1"),attr(path,"physics:diagonalInertia","float3","(1,1,1)")];
    if kinematic {ops.push(attr(path,"physics:kinematicEnabled","bool","true"));}ops
}
fn joint(name,first,second) {
    let path=PREFIX+name;
    [#{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"SixthHinge"+name,type_name:"PhysicsRevoluteJoint"}},
     attr(path,"physics:axis","token","\"X\""),
     #{SetRelationship:#{edit_target:"@runtime@",path:path,name:"physics:body0",targets:[PREFIX+first]}},
     #{SetRelationship:#{edit_target:"@runtime@",path:path,name:"physics:body1",targets:[PREFIX+second]}}]
}
fn port(name,key) {query("GetPort",#{api_id:find_path(PREFIX+name),name:key}).value}
fn x_axis(name) {
    let w=port(name,"quat_w");let x=port(name,"quat_x");let y=port(name,"quat_y");let z=port(name,"quat_z");
    [1.0-2.0*(y*y+z*z),2.0*(x*y+w*z),2.0*(x*z-w*y)]
}
fn near(actual,expected) {actual!=() && abs(actual-expected)<0.0001}
fn same_axis(first,second) {
    let a=x_axis(first);let b=x_axis(second);
    near(a[0],b[0]) && near(a[1],b[1]) && near(a[2],b[2])
}
fn linked(name) {
    if find_path(PREFIX+name)<0 {return false;}
    let binding=query("QueryUsdPrim",#{path:PREFIX+name,topology:true}).topology.binding;
    binding.physics_joint_link!=() && !binding.physics_joint_pending
}
fn finish(state,error) {
    if state.done {return state;}
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;report_verdict(state.checks,"SIMULATION CORE SIXTH HINGE","SIMULATION_CORE_SIXTH_HINGE");state
}
fn on_start(me,ctx) {
    this=#{doc:ctx.doc_id,frames:0,pending:cmd("SetTimeTransport",#{playing:false}).id,
        checks:[],done:false,recent:false,awaited:"pause command receipt",next_wait:"positive hinge authoring"};
}
fn on_event(me,event,ctx) {
    if event.name=="physics-joint-seating-invalid" && event.severity=="Error"
        && event.value.contains(PREFIX+"InvalidJoint") {this.recent=true;}
}
fn receipt_ready(me) {
    this.done || this.pending==() || command_result(this.pending).status!="pending"
}
fn accept_hinge_command(me) {
    if this.done {return;}
    try {
        let result=command_result(this.pending);
        if !result.ok {throw("hinge fixture command rejected: "+result);}
        this.pending=();this.awaited=this.next_wait;
    } catch(error) {this=finish(this,error);}
}
fn author_valid_hinges(me) {
    if this.done {return;}
    try {
        let ops=[];
        for spec in [["Reference",[0.0,0.0,0.0],true],["Twist",[60.0,0.0,0.0],true],
          ["Moving",[60.0,90.0,0.0],false],["Reverse",[60.0,90.0,0.0],false],
          ["Free",[60.0,0.0,0.0],false],["Basis",[0.0,0.0,0.0],false]] {
            ops=append(ops,body(spec[0],spec[1],spec[2]));
        }
        ops.push(attr(PREFIX+"Free","physics:angularVelocity","vector3f","(30,0,0)"));
        for name in ["Moving","Reverse"] {
            ops.push(attr(PREFIX+name,"physics:angularVelocity","vector3f","(30,0,0)"));
        }
        for spec in [["TwistJoint","Reference","Twist"],["MovingJoint","Reference","Moving"],
          ["ReverseJoint","Reverse","Reference"],["FreeJoint","Reference","Free"],["BasisJoint","Reference","Basis"]] {
            ops=append(ops,joint(spec[0],spec[1],spec[2]));
        }
        ops.push(attr(PREFIX+"BasisJoint","physics:localRot0","quatf","(0.70710677,0,0.70710677,0)"));
        this.awaited="positive hinge command receipt";this.next_wait="native admission of all five valid hinges";
        this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;
    } catch(error) {this=finish(this,error);}
}
fn valid_hinges_ready(me) {
    if this.done {return true;}
    try {
        if !query("GetReadiness").ready {return false;}
        for name in ["TwistJoint","MovingJoint","ReverseJoint","FreeJoint","BasisJoint"] {
            if !linked(name) {return false;}
        }
        return true;
    } catch(error) {this=finish(this,error);return true;}
}
fn check_valid_hinges(me) {
    if this.done {return;}
    try {
        let ready=query("GetReadiness");
        this.checks.push(t_true(near(port("Twist","quat_x"),0.5) && near(port("Twist","quat_w"),sqrt(3.0)/2.0),"prescribed legal hinge twist retains its authored pose"));
        this.checks.push(t_true(near(port("TwistJoint","angle"),PI()/3.0),"aligned prescribed frames retain their nonzero free coordinate"));
        this.checks.push(t_true(same_axis("Reference","Moving"),"movable endpoint 1 seats its constrained hinge axis"));
        this.checks.push(t_true(same_axis("Reference","Reverse"),"reversed movable endpoint 0 seats its constrained hinge axis"));
        this.checks.push(t_true(near(port("MovingJoint","angle"),PI()/3.0) && near(port("ReverseJoint","angle"),-PI()/3.0),
            "axis seating preserves authored free twist with the correct endpoint sign"));
        this.checks.push(t_true(near(query("QueryPhysicsState",#{id:find_path(PREFIX+"Moving")}).angular_velocity_radps[0],PI()/6.0)
            && near(query("QueryPhysicsState",#{id:find_path(PREFIX+"Reverse")}).angular_velocity_radps[0],PI()/6.0),
            "axis seating transports the moving endpoint's free angular rate in both endpoint orders: "+query("QueryPhysicsState",#{id:find_path(PREFIX+"Moving")}).angular_velocity_radps+" / "+query("QueryPhysicsState",#{id:find_path(PREFIX+"Reverse")}).angular_velocity_radps));
        this.checks.push(t_true(near(port("Reference","quat_w"),1.0),"seating preserves the prescribed reference orientation"));
        this.checks.push(t_true(near(port("FreeJoint","angle"),PI()/3.0),"dynamic legal twist remains free at admission"));
        this.checks.push(t_true(near(query("QueryPhysicsState",#{id:find_path(PREFIX+"Free")}).angular_velocity_radps[0],PI()/6.0),"authored hinge-axis angular velocity remains free"));
        let axis=x_axis("Basis");
        this.checks.push(t_true(near(axis[0],0.0) && near(axis[2],-1.0),"seating uses the authored local frame's world hinge axis"));
        this.checks.push(t_true(clock_snapshot().physics_paused && !ready.faulted,"all valid hinge cases admit without a physics step"));
    } catch(error) {this=finish(this,error);}
}
fn author_invalid_hinge(me) {
    if this.done {return;}
    try {
        let ops=append(body("Invalid0",[0.0,0.0,0.0],true),body("Invalid1",[0.0,90.0,0.0],true));
        ops=append(ops,joint("InvalidJoint","Invalid0","Invalid1"));
        this.awaited="invalid hinge command receipt";this.next_wait="prescribed hinge rejection and Recent Events Error";
        this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;
    } catch(error) {this=finish(this,error);}
}
fn invalid_hinge_ready(me) {
    this.done || (query("GetReadiness").faulted && this.recent
        && find_path(PREFIX+"Invalid0")>=0 && find_path(PREFIX+"Invalid1")>=0)
}
fn check_invalid_hinge(me) {
    if this.done {return;}
    try {
        let ready=query("GetReadiness");
        this.checks.push(t_true(ready.fault.kind=="physics-joint-seating-invalid" && ready.fault.detail.contains("hinge-axis"),"orthogonal prescribed axes reject with a specific constrained-axis error"));
        this.checks.push(t_true(this.recent && ready.application_available && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),"hinge rejection reaches Recent Events and holds physics without crashing"));
        this.checks.push(t_true(near(port("Invalid0","quat_w"),1.0) && near(port("Invalid1","quat_y"),sqrt(0.5)),"rejected admission preserves both prescribed orientations"));
        this.checks.push(t_true(!linked("InvalidJoint"),"incompatible prescribed hinge never completes native admission"));
    } catch(error) {this=finish(this,error);}
}
fn report_hinge(me) {if !this.done {this=finish(this,());}}
fn task(me,ctx) {
    seq([
        wait_until(Fn("receipt_ready")),
        once(Fn("accept_hinge_command")),
        once(Fn("author_valid_hinges")),
        wait_until(Fn("receipt_ready")),
        once(Fn("accept_hinge_command")),
        wait_until(Fn("valid_hinges_ready")),
        once(Fn("check_valid_hinges")),
        once(Fn("author_invalid_hinge")),
        wait_until(Fn("receipt_ready")),
        once(Fn("accept_hinge_command")),
        wait_until(Fn("invalid_hinge_ready")),
        once(Fn("check_invalid_hinge")),
        once(Fn("report_hinge"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;}this.frames+=1;
    if this.frames>1600 {this=finish(this,"hinge admission timed out awaiting "+this.awaited);}
}
