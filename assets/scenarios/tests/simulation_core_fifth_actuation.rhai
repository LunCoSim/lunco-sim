// @timing presentation
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","QueryPhysicsState","GetPort","QueryUsdPrim","RuntimeDiagnostics","ListPorts"],required_inputs:[]}
}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn state(path) {query("QueryPhysicsState",#{id:find_path(path)})}
fn value(path,name) {
    let result=query("GetPort",#{api_id:find_path(path),name:name});
    if result.value==() {throw("required port is unavailable: "+path+"."+name+" "+result);}
    result.value
}
fn position(path) {[value(path,"position_x"),value(path,"position_y"),value(path,"position_z")]}
fn finite_vector(vector) {
    for component in vector {if !component.is_finite() {return false;}}
    true
}
fn finite_body(sample) {
    sample.computed_mass_finite && sample.computed_inertia_finite
        && finite_vector(sample.linear_velocity_mps) && finite_vector(sample.angular_velocity_radps)
}
fn finish(current,error) {
    if error!=() {current.checks.push(error.to_string());}
    current.done=true;
    report_verdict(current.checks,"SIMULATION CORE FIFTH ACTUATION "+current.mode,
        "SIMULATION_CORE_FIFTH_ACTUATION_"+current.mode);current
}
fn on_start(me,ctx) {
    this=#{done:false,elapsed:0.0,frames:0,phase:0,pending:(),checks:[],mode:ctx.mode,
        body:ctx.body_path,mount:ctx.mount_path,recent:false,saw_awake:false};
    if !["dual","mass_wake","inverse_mass_overflow","accumulation_overflow","inverse_inertia_overflow"].contains(this.mode) {
        this=finish(this,"unknown actuation fixture mode: "+this.mode);
    }
}
fn on_event(me,event,ctx) {
    if event.name=="cosim-native-force-range" && event.severity=="Error"
        && event.value.contains(this.body)
        && event.value.contains("force=") && event.value.contains("inverse_mass=")
        && event.value.contains("increment") {this.recent=true;}
}
fn dual_tick(current,ready) {
    let sample=state(current.body);let tick=clock_snapshot().sim_tick;
    if current.phase==0 {
        if !ready.ready || ready.world_hold || !finite_body(sample) {return current;}
        let rest=sample.linear_velocity_mps==[0.0,0.0,0.0] && sample.angular_velocity_radps==[0.0,0.0,0.0];
        current.checks.push(t_true(rest,"dual actuator fixture starts admitted at rest"));
        current.pending=cmd("SetPorts",#{target:find_path(current.mount),writes:[["torque_command",3.0]]});
        current.admit_tick=tick;current.phase=1;return current;
    }
    if current.phase==1 {
        let receipt=terminal(current.pending);if receipt.status=="pending" {return current;}
        if tick-current.admit_tick<2 {return current;}
        current.checks.push(t_true(receipt.ok==true,"torque-only command is admitted"));
        if receipt.ok!=true {throw("torque-only write failed: "+receipt);}
        current.tick=tick;current.before=sample;current.phase=2;return current;
    }
    if current.phase==2 {
        if tick-current.tick<12 {return current;}
        let force_port=false;let torque_port=false;
        for port in query("ListPorts",#{api_id:find_path(current.mount)}).ports {
            force_port=force_port || port.name=="force_command";
            torque_port=torque_port || port.name=="torque_command";
        }
        current.checks.push(t_true(force_port && torque_port,
            "one actuator prim exposes both independent force and torque input contracts"));
        let torque=sample.angular_velocity_radps[1]-current.before.angular_velocity_radps[1];
        let translation=sample.linear_velocity_mps[1]-current.before.linear_velocity_mps[1];
        current.checks.push(t_true(torque>0.05 && abs(translation)<0.000001,
            "torque-only demand produces angular acceleration without linear force"));
        current.pending=cmd("SetPorts",#{target:find_path(current.mount),writes:[["force_command",2.0],["torque_command",0.0]]});
        current.admit_tick=tick;current.phase=3;return current;
    }
    if current.phase==3 {
        let receipt=terminal(current.pending);if receipt.status=="pending" {return current;}
        if tick-current.admit_tick<2 {return current;}
        current.checks.push(t_true(receipt.ok==true,"force-only demand and explicit torque release are admitted"));
        if receipt.ok!=true {throw("force-only write failed: "+receipt);}
        current.tick=tick;current.before=sample;current.phase=4;return current;
    }
    if current.phase==4 {
        if tick-current.tick<12 {return current;}
        let force=sample.linear_velocity_mps[1]-current.before.linear_velocity_mps[1];
        let angular=sample.angular_velocity_radps[1]-current.before.angular_velocity_radps[1];
        current.checks.push(t_true(force>0.05 && abs(angular)<0.00001,
            "force-only demand produces translation without another torque increment"));
        current.pending=cmd("SetPorts",#{target:find_path(current.mount),writes:[["force_command",1.0],["torque_command",2.0]]});
        current.admit_tick=tick;current.phase=5;return current;
    }
    if current.phase==5 {
        let receipt=terminal(current.pending);if receipt.status=="pending" {return current;}
        if tick-current.admit_tick<2 {return current;}
        current.checks.push(t_true(receipt.ok==true,"mixed force and torque batch is admitted"));
        if receipt.ok!=true {throw("mixed write failed: "+receipt);}
        current.tick=tick;current.before=sample;current.phase=6;return current;
    }
    if tick-current.tick<12 {return current;}
    let force=sample.linear_velocity_mps[1]-current.before.linear_velocity_mps[1];
    let angular=sample.angular_velocity_radps[1]-current.before.angular_velocity_radps[1];
    current.checks.push(t_true(force>0.05 && angular>0.05,"mixed demand supplies both independent physical effects"));
    current.checks.push(t_true(abs(angular-2.0*force)<0.0001,
        "mixed acceleration increments retain the distinct force-one and torque-two demands"));
    current.checks.push(t_true(ready.ready && !ready.faulted && finite_body(sample),
        "independent actuator application remains finite and admitted"));
    finish(current,())
}
fn sleep_tick(current,ready) {
    let sample=state(current.body);let tick=clock_snapshot().sim_tick;
    if current.phase==0 {
        if !ready.ready || ready.world_hold || !sample.sleeping {return current;}
        current.before_y=value(current.body,"position_y");
        current.checks.push(t_true(sample.mass_kg==1.0 && finite_body(sample),
            "spring-supported one-kilogram body reaches natural native sleep"));
        current.checks.push(t_true(abs(current.before_y-29.9)<0.02,
            "initial sleeping position matches the supported one-kilogram equilibrium"));
        current.pending=cmd("SetPorts",#{target:find_path(current.body),writes:[["mass",5.0]]});
        current.phase=1;return current;
    }
    if current.phase==1 {
        let receipt=terminal(current.pending);if receipt.status=="pending" {return current;}
        current.checks.push(t_true(receipt.ok==true,"live mass input change is admitted"));
        if receipt.ok!=true {throw("mass write failed: "+receipt);}
        current.tick=tick;current.phase=2;
    }
    current.saw_awake=current.saw_awake || (sample.mass_kg==5.0 && !sample.sleeping);
    if tick-current.tick<180 {return current;}
    if !sample.sleeping {return current;}
    let after_y=value(current.body,"position_y");
    current.checks.push(t_true(current.saw_awake,"admitted mass change wakes the sleeping body/island"));
    current.checks.push(t_true(sample.mass_kg==5.0 && finite_body(sample),"new mass and complete native inertia remain finite"));
    current.checks.push(t_true(abs(after_y-29.5)<0.02 && current.before_y-after_y>0.3,
        "the awakened body reaches the new five-kilogram spring equilibrium: "+after_y));
    current.checks.push(t_true(sample.sleeping,"the changed body can naturally sleep again after settling"));
    current.checks.push(t_true(ready.ready && !ready.world_hold && !ready.faulted,
        "live mass replacement leaves simulation readiness healthy"));
    finish(current,())
}
fn range_tick(current,ready) {
    let sample=state(current.body);
    if current.phase==0 {
        if !ready.ready || ready.world_hold || !finite_body(sample) {return current;}
        current.before=sample;current.before_position=position(current.body);
        current.checks.push(t_true(sample.body_mode=="dynamic" && sample.mass_kg>0.0,
            "native force fixture begins as a valid admitted dynamic body"));
        let expected=if current.mode=="inverse_mass_overflow" {0.1} else {1.0};
        let candidate=abs(sample.mass_kg-expected)<0.00000001;
        if current.mode=="inverse_inertia_overflow" {candidate=candidate && sample.inertia_principal_kgm2[0]<0.000000001;}
        current.checks.push(t_true(candidate && sample.computed_inertia_finite,
            "positive finite authored mass/inertia is admitted before the invalid force transaction"));
        let writes=[["force_x",3.0e38]];
        if current.mode=="accumulation_overflow" {writes.push(["force_local_x",3.0e38]);}
        if current.mode=="inverse_inertia_overflow" {writes=[["torque_y",3.0e38]];}
        current.pending=cmd("SetPorts",#{target:find_path(current.body),writes:writes});
        current.phase=1;return current;
    }
    if current.phase==1 {
        let receipt=terminal(current.pending);if receipt.status=="pending" {return current;}
        if receipt.ok!=true {throw("finite scalar inputs did not reach the native transaction owner: "+receipt);}
        if !ready.faulted || !current.recent || !clock_snapshot().physics_paused {return current;}
        current.checks.push(t_true(receipt.ok==true,"finite individual commands reach the checked force batch owner"));
        current.checks.push(t_true(ready.fault.kind=="cosim-native-force-range" && ready.fault.subject==current.body,
            "overflow raises the structured native force transaction fault"));
        let detail=ready.fault.detail;
        current.checks.push(t_true(detail.contains("force=") && detail.contains("inverse_mass=") && detail.contains("increment"),
            "failure reports actual force/torque, inverse mass and the overflowing increment"));
        current.checks.push(t_true(current.recent,"Recent Events includes an Error with the arithmetic failure operands"));
        let holds=clock_snapshot().physics_hold_reasons;
        current.checks.push(t_true(clock_snapshot().physics_paused && holds.contains("runtime-safety-failure"),
            "invalid force transaction holds native integration at its owner"));
        let unchanged=sample.linear_velocity_mps==current.before.linear_velocity_mps
            && sample.angular_velocity_radps==current.before.angular_velocity_radps;
        current.checks.push(t_true(finite_body(sample) && unchanged && position(current.body)==current.before_position,
            "rejection leaves native pose and velocities finite and unchanged"));
        current.held_tick=clock_snapshot().sim_tick;current.held_frame=current.frames;
        cmd("SetTimeTransport",#{playing:true});current.phase=2;return current;
    }
    if current.frames-current.held_frame<10 {return current;}
    current.checks.push(t_true(clock_snapshot().sim_tick==current.held_tick,
        "attempting resume cannot integrate the rejected arithmetic"));
    let retained=sample.mass_kg==current.before.mass_kg && sample.inertia_tensor_kgm2==current.before.inertia_tensor_kgm2;
    current.checks.push(t_true(ready.application_available && ready.faulted && retained && finite_body(sample),
        "the process stays available with the valid native mass/inertia candidate intact"));
    finish(current,())
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;this.elapsed+=execution_context().delta_seconds;
    if this.elapsed>90.0 {this=finish(this,"actuation timed out at phase "+this.phase+" mode="+this.mode);return;}
    try {
        let ready=query("GetReadiness");
        if this.mode=="dual" {
            if ready.faulted {throw("valid dual fixture faulted: "+ready);}
            this=dual_tick(this,ready);
        } else if this.mode=="mass_wake" {
            if ready.faulted {throw("valid mass fixture faulted: "+ready);}
            this=sleep_tick(this,ready);
        } else {this=range_tick(this,ready);}
    } catch(error) {this=finish(this,error);}
}
