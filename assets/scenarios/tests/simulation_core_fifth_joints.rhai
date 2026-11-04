// @timing presentation
const PREFIX="/World/Fifth";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryUsdPrim","QueryPhysicsState","GetPort"],required_inputs:[]}
}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn body(name,x,mode) {
    let path=PREFIX+name;
    let ops=[#{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"Fifth"+name,type_name:"Xform",reference:()}},
      #{SetApiSchemas:#{edit_target:"@runtime@",path:path,schemas:["PhysicsRigidBodyAPI","PhysicsMassAPI"]}},
      #{SetTranslate:#{edit_target:"@runtime@",path:path,value:[x,0.0,0.0]}},
      attr(path,"physics:mass","float","1"),
      attr(path,"physics:diagonalInertia","float3","(1,1,1)")];
    if mode=="static" {ops.push(attr(path,"physics:rigidBodyEnabled","bool","false"));}
    if mode=="kinematic" {ops.push(attr(path,"physics:kinematicEnabled","bool","true"));}
    ops
}
fn joint(name,kind,body0,body1,lower,upper) {
    let path=PREFIX+name;
    let ops=[#{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"Fifth"+name,type_name:kind,reference:()}}];
    if body0!="" {ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:path,name:"physics:body0",targets:[PREFIX+body0]}});}
    if body1!="" {ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:path,name:"physics:body1",targets:[PREFIX+body1]}});}
    if kind=="PhysicsPrismaticJoint" {
        ops.push(attr(path,"physics:axis","token","\"X\""));
        ops.push(attr(path,"physics:lowerLimit","float",lower.to_string()));
        ops.push(attr(path,"physics:upperLimit","float",upper.to_string()));
    }
    if kind=="PhysicsDistanceJoint" {
        ops.push(attr(path,"physics:minDistance","float",lower.to_string()));
        ops.push(attr(path,"physics:maxDistance","float",upper.to_string()));
    }
    ops
}
fn append(ops,more) {for op in more {ops.push(op);}ops}
fn x(name) {
    let id=find_path(PREFIX+name);if id<0 {return ();}
    query("GetPort",#{api_id:id,name:"position_x"}).value
}
fn linked(name) {
    let binding=query("QueryUsdPrim",#{path:PREFIX+name,topology:true}).topology.binding;
    binding.physics_joint_link!=() && !binding.physics_joint_pending
}
fn near(actual,expected) {actual!=() && abs(actual-expected)<0.0001}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FIFTH JOINTS","SIMULATION_CORE_FIFTH_JOINTS");state
}
fn on_start(me,ctx) {
    this=#{doc:ctx.doc_id,phase:0,frames:0,done:false,checks:[],recent:false,
      pending:cmd("SetTimeTransport",#{playing:false}).id};
}
fn on_event(me,event,ctx) {
    if event.name=="physics-joint-seating-invalid" && event.severity=="Error"
        && event.value.contains(PREFIX+"PrescribedJoint") {this.recent=true;}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1600 {this.checks.push("joint admission timed out at phase "+this.phase);this=finish(this);return;}
    try {
        if this.pending!=() {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if !result.ok {throw("joint fixture command rejected: "+result);}
            this.pending=();
        }
        if this.phase==0 {
            let ops=[];
            for spec in [["World0",10.0,"dynamic"],["World1",10.0,"dynamic"],
              ["Static",2.0,"static"],["StaticFollower",10.0,"dynamic"],
              ["Kinematic",3.0,"kinematic"],["KinematicFollower",10.0,"dynamic"],
              ["Slider",5.0,"dynamic"],["SliderLimit",8.0,"dynamic"],
              ["ReverseSlider",5.0,"dynamic"],["Distance",5.0,"dynamic"],
              ["DistanceLimit",8.0,"dynamic"],["MovingSlider",5.0,"dynamic"]] {
                ops=append(ops,body(spec[0],spec[1],spec[2]));
            }
            ops.push(attr(PREFIX+"MovingSlider","physics:velocity","vector3f","(1,0,0)"));
            for spec in [["World0Joint","PhysicsFixedJoint","","World0",0.0,0.0],
              ["World1Joint","PhysicsFixedJoint","World1","",0.0,0.0],
              ["StaticJoint","PhysicsFixedJoint","StaticFollower","Static",0.0,0.0],
              ["KinematicJoint","PhysicsFixedJoint","KinematicFollower","Kinematic",0.0,0.0],
              ["SliderJoint","PhysicsPrismaticJoint","","Slider",4.0,6.0],
              ["SliderLimitJoint","PhysicsPrismaticJoint","","SliderLimit",4.0,6.0],
              ["ReverseSliderJoint","PhysicsPrismaticJoint","ReverseSlider","",-6.0,-4.0],
              ["DistanceJoint","PhysicsDistanceJoint","","Distance",4.0,6.0],
              ["DistanceLimitJoint","PhysicsDistanceJoint","","DistanceLimit",4.0,6.0],
              ["MovingSliderJoint","PhysicsPrismaticJoint","","MovingSlider",-10.0,10.0]] {
                ops=append(ops,joint(spec[0],spec[1],spec[2],spec[3],spec[4],spec[5]));
            }
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=1;return;
        }
        let ready=query("GetReadiness");
        if this.phase==1 {
            if !ready.ready {return;}
            for name in ["World0Joint","World1Joint","StaticJoint","KinematicJoint",
              "SliderJoint","SliderLimitJoint","ReverseSliderJoint","DistanceJoint",
              "DistanceLimitJoint","MovingSliderJoint"] {if !linked(name) {return;}}
            this.checks.push(t_true(near(x("World0"),0.0),"world endpoint 0 stays at world origin"));
            this.checks.push(t_true(near(x("World1"),0.0),"world endpoint 1 stays at world origin"));
            this.checks.push(t_true(near(x("Static"),2.0),"static endpoint 1 keeps its prescribed pose"));
            this.checks.push(t_true(near(x("StaticFollower"),2.0),"dynamic endpoint 0 seats against static endpoint 1"));
            this.checks.push(t_true(near(x("Kinematic"),3.0),"kinematic endpoint 1 keeps its prescribed pose"));
            this.checks.push(t_true(near(x("KinematicFollower"),3.0),"dynamic endpoint 0 seats against kinematic endpoint 1"));
            this.checks.push(t_true(near(x("Slider"),5.0),"legal nonzero prismatic coordinate survives admission"));
            this.checks.push(t_true(near(x("SliderLimit"),6.0),"prismatic correction stops at the nearest allowed coordinate"));
            this.checks.push(t_true(near(x("ReverseSlider"),5.0),"swapped prismatic endpoints retain the signed allowed coordinate"));
            this.checks.push(t_true(near(x("Distance"),5.0),"distance interval preserves legal nonzero separation"));
            this.checks.push(t_true(near(x("DistanceLimit"),6.0),"distance correction stops at the nearest allowed separation"));
            let velocity=query("QueryPhysicsState",#{id:find_path(PREFIX+"MovingSlider")}).linear_velocity_mps;
            this.checks.push(t_true(near(x("MovingSlider"),5.0) && near(velocity[0],1.0),"prismatic free coordinate and authored free velocity survive paused admission"));
            this.checks.push(t_true(!ready.faulted && clock_snapshot().physics_paused,"positive constraints admit without advancing physics"));
            let ops=append(body("Prescribed0",30.0,"kinematic"),body("Prescribed1",31.0,"kinematic"));
            ops=append(ops,joint("PrescribedJoint","PhysicsFixedJoint","Prescribed0","Prescribed1",0.0,0.0));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=2;return;
        }
        if this.phase==2 {
            if !ready.faulted || !this.recent {return;}
            this.checks.push(t_true(ready.fault.kind=="physics-joint-seating-invalid" && ready.application_available,"incompatible prescribed poses fail visibly without crashing"));
            this.checks.push(t_true(this.recent && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),"prescribed constraint error reaches Recent Events and holds physics"));
            this.checks.push(t_true(near(x("Prescribed0"),30.0) && near(x("Prescribed1"),31.0),"rejected admission mutates neither prescribed endpoint"));
            this=finish(this);
        }
    } catch(error) {this.checks.push(error.to_string());this=finish(this);}
}
