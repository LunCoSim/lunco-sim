// @timing presentation
const HOST="/World/Host";
const SOCKET=HOST+"/Socket";
const FOLLOWER="/World/Follower";
const JOINT="/World/SocketJoint";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryUsdPrim","GetPort"],required_inputs:[]}
}
fn position() {
    let id=find_path(FOLLOWER);if id<0 {return ();}
    [query("GetPort",#{api_id:id,name:"position_x"}).value,
     query("GetPort",#{api_id:id,name:"position_y"}).value,
     query("GetPort",#{api_id:id,name:"position_z"}).value]
}
fn near(actual,expected) {
    if actual==() {return false;}
    for i in 0..3 {if actual[i]==() || abs(actual[i]-expected[i])>0.0001 {return false;}}true
}
fn linked() {
    let binding=query("QueryUsdPrim",#{path:JOINT,topology:true}).topology.binding;
    binding.physics_joint_link!=() && !binding.physics_joint_pending
}
fn apply(doc,op) {cmd("ApplyUsdOps",#{doc_id:doc,ops:[op]}).id}
fn finish(state) {
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FOURTH JOINTS","SIMULATION_CORE_FOURTH_JOINTS");state
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,pending:(),checks:[],recent:false,doc:ctx.doc_id};
}
fn on_event(me,event,ctx) {
    if event.name=="usd-physics-joint-invalid" && event.severity=="Error"
        && event.value.contains(JOINT) {this.recent=true;}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1200 {this.checks.push("joint refresh timed out at phase "+this.phase+" position="+position());this=finish(this);return;}
    try {
        let ready=query("GetReadiness");
        if this.pending!=() {
            let result=command_result(this.pending);if result.status=="pending" {return;}
            if !result.ok {throw("joint frame edit rejected: "+result);}
            this.pending=();
        }
        if this.phase==0 {
            if !ready.ready || !near(position(),[2.0,41.0,0.0]) {return;}
            this.vehicle=find_path("/World/Vehicle");
            this.checks.push(t_true(!ready.faulted && near(position(),[2.0,41.0,0.0]),"scaled descendant starts at the authored native endpoint"));
            this.checks.push(t_true(linked(),"initial native joint is admitted"));
            this.pending=apply(this.doc,#{SetTranslate:#{edit_target:"@runtime@",path:SOCKET,value:[2.0,0.0,0.0]}});this.phase=1;return;
        }
        if this.phase==1 {
            if !ready.ready || !near(position(),[4.0,41.0,0.0]) {return;}
            this.checks.push(t_true(near(position(),[4.0,41.0,0.0]),"socket-only translation reaches the native follower"));
            this.checks.push(t_true(linked(),"translated joint completes native readmission"));
            this.checks.push(t_true(find_path("/World/Vehicle")==this.vehicle,"joint refresh preserves unrelated projected identities"));
            this.pending=apply(this.doc,#{SetRotate:#{edit_target:"@runtime@",path:SOCKET,value:[0.0,0.0,0.0]}});this.phase=2;return;
        }
        if this.phase==2 {
            if !ready.ready || !near(position(),[5.0,40.0,0.0]) {return;}
            this.checks.push(t_true(near(position(),[5.0,40.0,0.0]),"socket rotation transports its nonzero native anchor"));
            this.checks.push(t_true(linked(),"rotated joint completes native readmission"));
            this.pending=apply(this.doc,#{SetScale:#{edit_target:"@runtime@",path:SOCKET,value:[2.0,3.0,1.0]}});this.phase=3;return;
        }
        if this.phase==3 {
            if !ready.ready || !near(position(),[6.0,40.0,0.0]) {return;}
            this.checks.push(t_true(near(position(),[6.0,40.0,0.0]),"descendant nonuniform scale reaches the native anchor"));
            this.checks.push(t_true(linked(),"scaled descendant joint completes native readmission"));
            this.pending=apply(this.doc,#{SetScale:#{edit_target:"@runtime@",path:HOST,value:[3.0,2.0,1.0]}});this.phase=4;return;
        }
        if this.phase==4 {
            if !ready.ready || !near(position(),[9.0,40.0,0.0]) {return;}
            this.checks.push(t_true(near(position(),[9.0,40.0,0.0]),"body scale refreshes the descendant endpoint exactly once"));
            this.checks.push(t_true(linked(),"body-scaled joint completes native readmission"));
            this.pending=apply(this.doc,#{SetScale:#{edit_target:"@runtime@",path:SOCKET,value:[0.0,0.0,0.0]}});this.phase=5;return;
        }
        if this.phase==5 {
            if !ready.faulted || !this.recent || !clock_snapshot().physics_paused {return;}
            this.checks.push(t_true((ready.fault.kind=="usd-physics-joint-invalid"
                || ready.fault.kind=="usd-transform-projection-invalid") && ready.application_available,
                "singular endpoint fails visibly without crashing"));
            this.checks.push(t_true(this.recent && clock_snapshot().physics_hold_reasons.contains("runtime-safety-failure"),
                "Recent Events names the invalid joint and physics is held"));
            this.checks.push(t_true(near(position(),[9.0,40.0,0.0]),"invalid replacement preserves the previous native body state"));
            this=finish(this);
        }
    } catch(error) {this.checks.push(error.to_string());this=finish(this);}
}
