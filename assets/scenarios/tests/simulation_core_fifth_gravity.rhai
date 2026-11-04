// @timing presentation
const SCENE="/World/Physics";
const BODY="/World/ForceBody";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryPhysicsState","GetPort"],required_inputs:[]}
}
fn sample() {query("QueryPhysicsState",#{id:find_path(BODY)})}
fn near_vector(actual,expected) {
    if actual==() {return false;}
    for axis in 0..3 {if abs(actual[axis]-expected[axis])>0.0001 {return false;}}true
}
fn edit(doc,name,kind,value) {
    cmd("ApplyUsdOps",#{doc_id:doc,ops:[#{SetAttribute:#{edit_target:"@runtime@",path:SCENE,name:name,type_name:kind,value:value}}]}).id
}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FIFTH GRAVITY","SIMULATION_CORE_FIFTH_GRAVITY");state
}
fn on_start(me,ctx) {
    this=#{doc:ctx.doc_id,phase:0,pending:(),frames:0,done:false,checks:[],tick:0,recent:false,velocity:()};
}
fn on_event(me,event,ctx) {
    if event.name=="usd-physics-gravity-invalid" && event.severity=="Error"
        && event.value.contains(SCENE) && event.value.contains("physics:gravityMagnitude") {this.recent=true;}
}
fn on_tick(me,ctx) {
    if this.done {return;}this.frames+=1;
    if this.frames>1600 {this.checks.push("live gravity timed out at phase "+this.phase);this=finish(this);return;}
    try {
        if this.pending!=() {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if !result.ok {throw("gravity edit rejected: "+result);}
            this.pending=();this.tick=clock_snapshot().sim_tick;
        }
        let ready=query("GetReadiness");let body=sample();
        if this.phase==4 {
            if !ready.faulted || !this.recent || !clock_snapshot().physics_paused {return;}
            this.checks.push(t_true(ready.fault.kind=="usd-physics-gravity-invalid" && ready.application_available,"invalid live gravity faults without crashing"));
            this.checks.push(t_true(this.recent && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),"Recent Events names the rejected gravity property and physics is held"));
            this.checks.push(t_true(near_vector(body.constant_linear_acceleration_mps2,[10.0,0.0,0.0]),"invalid gravity retains the last valid native acceleration"));
            this.checks.push(t_true(body.linear_velocity_mps[0].is_finite() && body.linear_velocity_mps[1].is_finite(),"rejected gravity leaves native velocity finite"));
            this=finish(this);return;
        }
        if !ready.ready || ready.world_hold {return;}
        if this.phase==0 {
            if body.constant_linear_acceleration_mps2==() {return;}
            this.checks.push(t_true(near_vector(body.constant_linear_acceleration_mps2,[0.0,0.0,0.0]),"admitted baseline gravity is zero"));
            this.velocity=body.linear_velocity_mps;
            this.pending=edit(this.doc,"physics:gravityMagnitude","float","10");
            cmd("SetTimeTransport",#{playing:true});this.phase=1;return;
        }
        if clock_snapshot().sim_tick-this.tick<8 {return;}
        if this.phase==1 {
            if !near_vector(body.constant_linear_acceleration_mps2,[0.0,-10.0,0.0]) {return;}
            this.checks.push(t_true(true,"info-only magnitude edit reaches native gravity after admission"));
            this.checks.push(t_true(body.linear_velocity_mps[1]<this.velocity[1]-0.01,"dynamic body responds to live gravity magnitude"));
            this.velocity=body.linear_velocity_mps;
            this.pending=edit(this.doc,"physics:gravityDirection","vector3f","(1,0,0)");this.phase=2;return;
        }
        if this.phase==2 {
            if !near_vector(body.constant_linear_acceleration_mps2,[10.0,0.0,0.0]) {return;}
            this.checks.push(t_true(true,"live direction uses the stage-to-canonical axes"));
            this.checks.push(t_true(body.linear_velocity_mps[0]>this.velocity[0]+0.01,"dynamic body responds along the new gravity direction"));
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[#{SetPrimDocumentation:#{edit_target:"@runtime@",path:SCENE,documentation:"gravity unchanged"}}]}).id;
            this.phase=3;return;
        }
        if this.phase==3 {
            this.checks.push(t_true(near_vector(body.constant_linear_acceleration_mps2,[10.0,0.0,0.0]) && !ready.faulted,"unrelated settings notice leaves native acceleration unchanged"));
            this.pending=edit(this.doc,"physics:gravityMagnitude","float","inf");this.phase=4;
        }
    } catch(error) {this.checks.push(error.to_string());this=finish(this);}
}
