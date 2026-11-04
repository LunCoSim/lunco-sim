// Read the admitted body-local tensor through its native query owner.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/World/Source/AuditMount"),
        find_path("/World/Target"),find_path("/World/Target/AuditMass")],entity_writes:[],
      query_reads:["GetPort","GetReadiness","RuntimeDiagnostics","QueryPhysicsState"],required_inputs:[]}
}
fn sample(entity,name) {
    let value=query("GetPort",#{api_id:entity,name:name});
    if value==() || value.ok==false || value.value==() {throw("native sample unavailable: "+name+" "+value);}
    value.value
}
fn on_start(me,ctx) {
    this.frames=0;this.done=false;this.fold_tick=();
    this.up_axis=if ctx.up_axis==() {"Y"} else {ctx.up_axis};
    if this.up_axis!="Y" && this.up_axis!="Z" {
        this.done=true;report_verdict(["fixture up_axis must be Y or Z"],
            "SIMULATION CORE PHYSICS","SIMULATION_CORE_PHYSICS");
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>600 {
        this.done=true;report_verdict(["native physical fixture did not settle"],
            "SIMULATION CORE PHYSICS","SIMULATION_CORE_PHYSICS");return;
    }
    try {
        let body=find_path("/World/Target");let mount=find_path("/World/Source/AuditMount");
        let ready=query("GetReadiness");
        if ready.faulted {throw("fixture faulted before native observations: "+ready);}
        let native=query("QueryPhysicsState",#{id:body});
        let inertia=native.inertia_tensor_kgm2;
        let local=get(mount,"ForceActuator.local_position");
        if inertia==() || local==() || !ready.ready || ready.world_hold
            || ready.pending_count!=0 {return;}
        if abs(sample(body,"mass")-6.0)>0.00001 {return;}
        if this.fold_tick==() {this.fold_tick=sim_tick();return;}
        if sim_tick()-this.fold_tick<2 {return;}
        let position=vec3_array(local);
        let diagonal=if this.up_axis=="Y" {[12.0+1.0/30.0,11.0,13.0+1.0/30.0]}
            else {[12.0+1.0/30.0,13.0+1.0/30.0,11.0]};
        let centre=if this.up_axis=="Y" {[0.0,1.0/30.0,0.0]} else {[0.0,0.0,-1.0/30.0]};
        let checks=[t_true(abs(sample(body,"mass")-6.0)<0.00001,"folded mass must be 6 kg")];
        for axis in 0..3 {
            checks.push(t_true(abs(position[axis]-if axis==0 {1.0} else {0.0})<0.000000001,
                "large-coordinate actuator body-local mount axis "+axis+" is "+position[axis]));
        }
        for axis in 0..3 {
            checks.push(t_true(abs(inertia[axis][axis]-diagonal[axis])<0.00003,
                "folded tensor diagonal "+axis+" is "+inertia[axis][axis]+" expected "+diagonal[axis]));
        }
        for pair in [[0,1],[0,2],[1,2]] {
            checks.push(t_true(abs(inertia[pair[0]][pair[1]])<0.00003,
                "folded tensor off-diagonal "+pair+" is "+inertia[pair[0]][pair[1]]));
        }
        for axis in 0..3 {
            let actual=sample(body,["com_x","com_y","com_z"][axis]);
            checks.push(t_true(abs(actual-centre[axis])<0.000001,
                "folded centre axis "+axis+" is "+actual+" expected "+centre[axis]));
        }
        checks.push(t_true(ready.ready && !ready.world_hold && ready.pending_count==0,
            "native mass and actuation remain admitted"));
        checks.push(t_true(query("RuntimeDiagnostics").errors==0,"native fixture reports a runtime error"));
        checks.push(t_true(query("QueryPhysicsState",#{id:find_path("/World/Target/AuditMass")}).body_mode==(),
            "a reduced mass contribution must not become an independently simulated body"));
        emit("SIMULATION_CORE_PHYSICS_EVIDENCE",#{up_axis:this.up_axis,local_mount:position,
            body_tensor:inertia,expected_diagonal:diagonal,expected_centre:centre});
        this.done=true;report_verdict(checks,"SIMULATION CORE PHYSICS","SIMULATION_CORE_PHYSICS");
    } catch(error) {
        this.done=true;report_verdict(["native physics observation failed: "+error],
            "SIMULATION CORE PHYSICS","SIMULATION_CORE_PHYSICS");
    }
}
