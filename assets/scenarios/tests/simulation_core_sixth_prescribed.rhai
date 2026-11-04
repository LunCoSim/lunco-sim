// @timing presentation
// Native body mode changes use the existing typed USD/document projection.
fn simulation_dependencies(me,ctx) {
    let body=find_path(ctx.body_path);let actuator=find_path(ctx.actuator_path);
    #{modelica_entities:[],entity_reads:[body,actuator],entity_writes:[body,actuator],
      query_reads:["QueryPhysicsState","GetReadiness","RuntimeDiagnostics"],required_inputs:[]}
}
fn set_mode(doc,path,kinematic) {
    cmd("ApplyUsdOps",#{doc_id:doc,ops:[#{SetAttribute:#{edit_target:"@runtime@",path:path,
        name:"physics:kinematicEnabled",type_name:"bool",value:if kinematic {"true"} else {"false"}}}]})
}
fn finish(state,error) {
    if state.done {return state;}
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;report_verdict(state.checks,"SIMULATION CORE SIXTH PRESCRIBED",
        "SIMULATION_CORE_SIXTH_PRESCRIBED");state
}
fn admitted() {
    let ready=query("GetReadiness");
    if ready.faulted {throw("force eligibility faulted the app: "+ready);}
    ready.ready && !ready.world_hold
}
fn body_state(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn dynamic_fixture_ready(me) {
    if this.done {return false;}
    try {return admitted() && body_state(this.ctx.body_path).body_mode=="dynamic";}
    catch(error) {this=finish(this,error);return false;}
}
fn start_driving(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let body=find_path(ctx.body_path);let actuator=find_path(ctx.actuator_path);
        this.checks.push(t_true(cmd("SetPorts",#{target:body,writes:[["force_x",6.0]]}).ok==true,
            "dynamic body force hold is admitted"));
        this.checks.push(t_true(cmd("SetPorts",#{target:actuator,writes:[["force_command",2.0]]}).ok==true,
            "dynamic mounted force hold is admitted"));
        this.mark=clock_snapshot().sim_tick;this.interval=3;this.waiting="three initial force ticks";
    } catch(error) {this=finish(this,error);}
}
fn interval_elapsed(me) {
    if this.done {return false;}
    try {return admitted() && clock_snapshot().sim_tick-this.mark>=this.interval;}
    catch(error) {this=finish(this,error);return false;}
}
fn capture_initial_velocity(me) {
    if this.done {return;}
    try {
        this.v0=body_state(this.ctx.body_path).linear_velocity_mps;
        this.mark=clock_snapshot().sim_tick;this.interval=4;this.waiting="initial four-tick acceleration sample";
    } catch(error) {this=finish(this,error);}
}
fn check_initial_and_prescribe(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let state=body_state(ctx.body_path);
        let elapsed=(clock_snapshot().sim_tick-this.mark).to_float()*clock_snapshot().fixed_dt_s;
        this.checks.push(t_true(abs(state.linear_velocity_mps[0]-this.v0[0]-6.0*elapsed)<0.0002,
            "body hold drives the admitted dynamic receiver"));
        this.checks.push(t_true(abs(state.linear_velocity_mps[1]-this.v0[1]-2.0*elapsed)<0.0002,
            "mounted hold drives its exact receiver"));
        let result=set_mode(ctx.doc_id,ctx.body_path,true);
        if result.id==() {throw("prescribed mode edit has no receipt");}
        this.pending=result.id;this.expected_mode="kinematic";this.waiting="prescribed edit receipt and projection";
    } catch(error) {this=finish(this,error);}
}
fn mode_edit_completed(me) {
    if this.done {return false;}
    try {
        if this.pending!=() {
            let result=command_result(this.pending);
            if result.status=="pending" {return false;}
            if result.ok!=true {throw("fixture mode edit rejected: "+result);}
            this.pending=();
        }
        return admitted() && body_state(this.ctx.body_path).body_mode==this.expected_mode;
    } catch(error) {this=finish(this,error);return false;}
}
fn reject_prescribed_inputs(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let body=find_path(ctx.body_path);let actuator=find_path(ctx.actuator_path);
        this.v0=body_state(ctx.body_path).linear_velocity_mps;this.mark=clock_snapshot().sim_tick;
        this.checks.push(t_true(true,"receiver entered prescribed motion"));
        let rejected=cmd("SetPorts",#{target:body,writes:[["force_x",12.0]]});
        this.checks.push(t_true(rejected.ok==false,"new body force rejects terminally on a prescribed receiver"));
        rejected=cmd("SetPorts",#{target:actuator,writes:[["force_command",3.0]]});
        this.checks.push(t_true(rejected.ok==false,"new mounted force rejects terminally on a prescribed receiver"));
        rejected=cmd("SetPorts",#{target:actuator,writes:[["torque_command",1.0]]});
        this.checks.push(t_true(rejected.ok==false,"new mounted torque rejects terminally on a prescribed receiver"));
        this.interval=5;this.waiting="five prescribed ticks without force accumulation";
    } catch(error) {this=finish(this,error);}
}
fn check_prescribed_and_restore(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let state=body_state(ctx.body_path);let ready=query("GetReadiness");
        this.checks.push(t_true(state.linear_velocity_mps==this.v0,
            "held forces do not accumulate during prescribed motion"));
        this.checks.push(t_true(!ready.faulted && !ready.world_hold,
            "prescribed interval and command rejection keep the world running"));
        let result=set_mode(ctx.doc_id,ctx.body_path,false);
        if result.id==() {throw("dynamic mode edit has no receipt");}
        this.pending=result.id;this.expected_mode="dynamic";this.waiting="dynamic edit receipt and projection";
    } catch(error) {this=finish(this,error);}
}
fn observe_restored_receiver(me) {
    if this.done {return;}
    try {
        this.v0=body_state(this.ctx.body_path).linear_velocity_mps;
        this.mark=clock_snapshot().sim_tick;this.interval=5;this.waiting="five restored dynamic ticks";
    } catch(error) {this=finish(this,error);}
}
fn check_restored_and_drive(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let body=find_path(ctx.body_path);let actuator=find_path(ctx.actuator_path);
        let state=body_state(ctx.body_path);
        this.checks.push(t_true(state.angular_velocity_radps==[0.0,0.0,0.0],
            "rejected torque did not reach the physical receiver"));
        this.checks.push(t_true(query("RuntimeDiagnostics").errors==0,
            "receiver transitions preserve structured runtime health"));
        this.checks.push(t_true(cmd("SetPorts",#{target:body,writes:[["force_x",6.0]]}).ok==true,
            "new dynamic projection admits a fresh body force intent"));
        this.checks.push(t_true(cmd("SetPorts",#{target:actuator,writes:[["force_command",2.0]]}).ok==true,
            "new dynamic projection admits a fresh mounted force intent"));
        this.v0=state.linear_velocity_mps;this.mark=clock_snapshot().sim_tick;
        this.interval=5;this.waiting="fresh five-tick acceleration sample";
    } catch(error) {this=finish(this,error);}
}
fn check_fresh_acceleration(me) {
    if this.done {return;}
    try {
        let state=body_state(this.ctx.body_path);
        let elapsed=(clock_snapshot().sim_tick-this.mark).to_float()*clock_snapshot().fixed_dt_s;
        this.checks.push(t_true(abs(state.linear_velocity_mps[0]-this.v0[0]-6.0*elapsed)<0.0002,
            "fresh body intent accelerates the current dynamic projection"));
        this.checks.push(t_true(abs(state.linear_velocity_mps[1]-this.v0[1]-2.0*elapsed)<0.0002,
            "fresh mounted intent accelerates its current receiver"));
        this=finish(this,());
    } catch(error) {this=finish(this,error);}
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,pending:(),checks:[],mark:0,ctx:ctx,interval:0,
        expected_mode:"dynamic",waiting:"dynamic fixture admission"};
}
fn task(me,ctx) {
    seq([
        wait_until(Fn("dynamic_fixture_ready")), once(Fn("start_driving")),
        wait_until(Fn("interval_elapsed")), once(Fn("capture_initial_velocity")),
        wait_until(Fn("interval_elapsed")), once(Fn("check_initial_and_prescribe")),
        wait_until(Fn("mode_edit_completed")), once(Fn("reject_prescribed_inputs")),
        wait_until(Fn("interval_elapsed")), once(Fn("check_prescribed_and_restore")),
        wait_until(Fn("mode_edit_completed")), once(Fn("observe_restored_receiver")),
        wait_until(Fn("interval_elapsed")), once(Fn("check_restored_and_drive")),
        wait_until(Fn("interval_elapsed")), once(Fn("check_fresh_acceleration"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1800 {this=finish(this,"prescribed receiver gate timed out awaiting "+this.waiting);}
}
fn on_stop(me,ctx) {
    if !this.done {this=finish(this,"prescribed receiver gate stopped awaiting "+this.waiting);}
}
