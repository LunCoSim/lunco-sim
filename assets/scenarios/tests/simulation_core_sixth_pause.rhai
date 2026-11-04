// The observer is outside the suspended ownership graph. Its Simulation clock
// callback pauses only after the moving axle and its Modelica owner are ready.
fn simulation_dependencies(me,ctx) {
    let reads=vessel_descendants(find_path(ctx.rover_path));
    for entity in vessel_descendants(find_path(ctx.physical_path)) {reads.push(entity);}
    reads.push(find_path(ctx.independent_path));
    let writes=[find_path(ctx.rover_path)];
    for corner in ["FL","FR","RL","RR"] {
        writes.push(find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner));
    }
    writes.push(find_path(ctx.physical_path+"/Suspension_FL/Wheel_FL"));
    #{modelica_entities:[find_path(ctx.model_path)],entity_reads:reads,
      entity_writes:writes,
      query_reads:["GetPort","QueryPhysicsState","GetReadiness","RuntimeDiagnostics"],required_inputs:[]}
}
fn sample(wheel,name) {
    let result=query("GetPort",#{api_id:wheel,name:name});
    if result.value==() {throw("paused wheel sample unavailable: "+name+" "+result);}
    result.value
}
fn wheel_state(wheel) {
    [get(wheel,"WheelRaycast.spin_velocity"),get(wheel,"WheelRaycast.spin_angle"),
     sample(wheel,"shaft_speed"),sample(wheel,"bearing_heat"),sample(wheel,"brake_heat"),
     sample(wheel,"tire_heat")]
}
fn finish(state,error) {
    if state.done {return state;}
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;report_verdict(state.checks,"SIMULATION CORE SIXTH PAUSE",
        "SIMULATION_CORE_SIXTH_PAUSE");state
}
fn admitted() {
    let ready=query("GetReadiness");
    if ready.faulted {throw("moving-wheel pause fixture faulted: "+ready);}
    ready.ready && !ready.world_hold
}
fn fixture_ready(me) {
    if this.done {return false;}
    try {return admitted();} catch(error) {this=finish(this,error);return false;}
}
fn start_drive(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;
        for corner in ["FL","FR","RL","RR"] {
            let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
            let result=cmd("SetPorts",#{target:wheel,writes:[["drive",40.0]]});
            if result.ok!=true {throw("moving raycast wheel command rejected: "+result);}
        }
        let control=find_path(ctx.physical_path+"/Suspension_FL/Wheel_FL");
        let result=cmd("SetPorts",#{target:control,writes:[["drive",40.0]]});
        if result.ok!=true {throw("unpaused physical control command rejected: "+result);}
        this.waiting="nonzero raycast and physical shaft motion";
    } catch(error) {this=finish(this,error);}
}
fn wheels_moving(me) {
    if this.done {return false;}
    try {
        if !admitted() {return false;}
        let ctx=this.ctx;
        for corner in ["FL","FR","RL","RR"] {
            let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
            if abs(sample(wheel,"shaft_speed"))<0.01 {return false;}
        }
        return abs(sample(find_path(ctx.physical_path+"/Suspension_FL/Wheel_FL"),"shaft_speed"))>=0.01;
    } catch(error) {this=finish(this,error);return false;}
}
fn pause_moving_object(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let model=find_path(ctx.model_path);
        if get(model,"ModelicaModel.current_time")==() {throw("fixture has no declared Modelica clock owner: "+ctx.model_path);}
        for corner in ["FL","FR","RL","RR"] {
            let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
            this.saved.push(wheel_state(wheel));
        }
        this.model_time=get(model,"ModelicaModel.current_time");
        this.independent_start=query("QueryPhysicsState",#{id:find_path(ctx.independent_path)}).linear_velocity_mps;
        this.control_speed=sample(find_path(ctx.physical_path+"/Suspension_FL/Wheel_FL"),"shaft_speed");
        this.checks.push(t_true(true,"all raycast wheels and the unpaused physical control were moving"));
        let paused=cmd("PauseSimulationObject",#{target:find_path(ctx.rover_path),condition:"test:moving-axle-pause",
            detail:"Freeze the admitted mechanical axle, outputs and coupled model while other physics continues"});
        this.checks.push(t_true(paused.ok==true,"moving coupled object pause is admitted at a completed boundary"));
        this.mark=sim_tick();this.waiting="eight independent fixed ticks after scoped pause";
    } catch(error) {this=finish(this,error);}
}
fn pause_interval_elapsed(me) {
    if this.done {return false;}
    try {return admitted() && sim_tick()-this.mark>=8;}
    catch(error) {this=finish(this,error);return false;}
}
fn inspect_pause(me) {
    if this.done {return;}
    try {
        let ctx=this.ctx;let rover=find_path(ctx.rover_path);let model=find_path(ctx.model_path);
        let independent=find_path(ctx.independent_path);
        let state=query("QueryPhysicsState",#{id:rover});
        this.checks.push(t_true(state.simulation_object_paused && state.rigid_body_disabled,
            "native receiver remains scoped and disabled"));
        let index=0;
        for corner in ["FL","FR","RL","RR"] {
            let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
            this.checks.push(t_true(wheel_state(wheel)==this.saved[index],
                "paused raycast angle/speed/shaft/heat remain exact for "+corner));index+=1;
        }
        this.checks.push(t_true(get(model,"ModelicaModel.current_time")==this.model_time
            && get(model,"ModelicaModel.paused")==true,"coupled Modelica clock remains exact"));
        this.checks.push(t_true(query("QueryPhysicsState",#{id:independent}).linear_velocity_mps!=this.independent_start,
            "independent dynamic physics advances during scoped pause"));
        let control=find_path(ctx.physical_path+"/Suspension_FL/Wheel_FL");
        this.checks.push(t_true(sample(control,"shaft_speed")!=this.control_speed,
            "unpaused physical wheel retains its mechanical integration"));
        this.checks.push(t_true(!query("GetReadiness").world_hold && query("RuntimeDiagnostics").errors==0,
            "scoped pause does not fault or hold the independent world"));
        this=finish(this,());
    } catch(error) {this=finish(this,error);}
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,checks:[],saved:[],mark:0,ctx:ctx,waiting:"fixture admission"};
}
fn task(me,ctx) {
    seq([
        wait_until(Fn("fixture_ready")), once(Fn("start_drive")),
        wait_until(Fn("wheels_moving")), once(Fn("pause_moving_object")),
        wait_until(Fn("pause_interval_elapsed")), once(Fn("inspect_pause"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1800 {this=finish(this,"moving-wheel pause observer timed out awaiting "+this.waiting);}
}
fn on_stop(me,ctx) {
    if !this.done {this=finish(this,"moving-wheel pause observer stopped awaiting "+this.waiting);}
}
