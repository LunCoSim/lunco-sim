// The API driver authors the fixture through document operations. Physical:
// dynamic deck constrained only vertically/rotationally, no tangential damping;
// body_paths contains every dynamic rover member and the deck. Raycast: static
// deck, gravity .001 m/s2 and 1 N/m springs produce positive sub-newton loads.
fn simulation_dependencies(me,ctx) {
    let reads=[];let writes=[];
    for path in ctx.body_paths {reads.push(find_path(path));}
    let rover=find_path("/World/FourthTire");reads.push(rover);
    for corner in ["FL","FR","RL","RR"] {
        let wheel=find_path("/World/FourthTire/Suspension_"+corner+"/Wheel_"+corner);
        reads.push(wheel);writes.push(wheel);
    }
    #{modelica_entities:[],entity_reads:reads,entity_writes:writes,
      query_reads:["GetPort","QueryPhysicsState","GetReadiness","RuntimeDiagnostics"],required_inputs:[]}
}
fn sample(entity,name) {
    let value=query("GetPort",#{api_id:entity,name:name});
    if value.value==() {throw("required tire port unavailable: "+name+" "+value);}
    value.value
}
fn momentum(paths,axis) {
    let total=0.0;
    for path in paths {
        let entity=find_path(path);if entity<0 {throw("momentum participant is missing: "+path);}
        let state=query("QueryPhysicsState",#{id:entity});
        if state.body_mode!="dynamic" {throw("momentum participant is not dynamic: "+path+" "+state);}
        total+=sample(entity,"mass")*state.linear_velocity_mps[axis];
    }
    total
}
fn finish(state,error) {
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;
    let suffix=if state.mode=="physical" {"PHYSICAL"} else {"RAYCAST"};
    report_verdict(state.checks,"SIMULATION CORE FOURTH SIGNALS TIRES "+suffix,
        "SIMULATION_CORE_FOURTH_SIGNALS_TIRES_"+suffix);
    state
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,mark:0,checks:[],mode:ctx.mode,
        saw_loaded:false,saw_force:false,last_sample:()};
    if ctx.mode!="physical" && ctx.mode!="raycast" {this=finish(this,"tire fixture mode must be physical or raycast");}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1800 {this=finish(this,"tire fixture timed out at phase "+this.phase+"; sample="+this.last_sample);return;}
    try {
        let ready=query("GetReadiness");if ready.faulted {throw("tire fixture faulted: "+ready);}
        if !ready.ready || ready.world_hold {return;}
        let rover=find_path("/World/FourthTire");
        let axis=2;
        let loaded=false;
        let low_load=false;
        let patch_force=false;
        let state=query("QueryPhysicsState",#{id:rover});
        this.last_sample=#{mode:state.body_mode,sleeping:state.sleeping,velocity:state.linear_velocity_mps,
            initialization_paused:state.physics_initialization_paused,contacts:state.wheel_contacts};
        if this.mode=="physical" {
            for corner in ["FL","FR","RL","RR"] {
                let wheel=find_path("/World/FourthTire/Suspension_"+corner+"/Wheel_"+corner);
                if wheel>=0 {
                    let contact=query("GetPort",#{api_id:wheel,name:"contact_force"});
                    if contact.value!=() && contact.value>0.0 {loaded=true;}
                }
            }
        } else {
            for contact in state.wheel_contacts {
                if contact.contact_valid && contact.normal_force_n>0.0 && contact.normal_force_n<1.0 {
                    low_load=true;loaded=true;
                    let force=contact.tire_force_n;
                    if abs(force[0])+abs(force[1])+abs(force[2])>0.0 {patch_force=true;}
                }
            }
        }
        if this.phase==0 {
            if !loaded {return;}
            this.p0=if this.mode=="physical" {momentum(ctx.body_paths,axis)} else {0.0};
            this.rover_v0=state.linear_velocity_mps[axis];
            if this.mode=="physical" {
                this.deck_v0=query("QueryPhysicsState",#{id:find_path(ctx.deck_path)}).linear_velocity_mps[axis];
            }
            this.checks.push(t_true(loaded,"native tire contact carries positive load"));
            for corner in ["FL","FR","RL","RR"] {
                let wheel=find_path("/World/FourthTire/Suspension_"+corner+"/Wheel_"+corner);
                let result=cmd("SetPorts",#{target:wheel,writes:[["drive",20.0]]});
                this.checks.push(t_true(result.ok==true,"drive torque admitted for "+corner));
            }
            this.mark=sim_tick();this.phase=1;return;
        }
        this.saw_loaded=this.saw_loaded || low_load;
        this.saw_force=this.saw_force || patch_force;
        if sim_tick()-this.mark<60 {return;}
        if this.mode=="physical" {
            let p1=momentum(ctx.body_paths,axis);
            let deck=query("QueryPhysicsState",#{id:find_path(ctx.deck_path)});
            this.checks.push(t_true(abs(deck.linear_velocity_mps[axis]-this.deck_v0)>1.0e-5,
                "dynamic support receives the tire reaction"));
            this.checks.push(t_true(abs(state.linear_velocity_mps[axis]-this.rover_v0)>1.0e-5,
                "driven rover responds to the contact force"));
            this.checks.push(t_true(abs(p1-this.p0)<ctx.momentum_tolerance,
                "internal tire forces conserve tangential momentum: initial="+this.p0+" final="+p1));
            emit("SIMULATION_CORE_FOURTH_TIRE_MOMENTUM",#{initial:this.p0,final:p1,
                deck_velocity:deck.linear_velocity_mps[axis],rover_velocity:state.linear_velocity_mps[axis]});
        } else {
            this.checks.push(t_true(this.saw_loaded,"positive sub-newton contact remains loaded"));
            this.checks.push(t_true(this.saw_force,"sub-newton contact supplies nonzero tire force"));
            this.checks.push(t_true(abs(state.linear_velocity_mps[axis]-this.rover_v0)>1.0e-7,
                "sub-newton tire force reaches the native chassis"));
        }
        this.checks.push(t_true(query("RuntimeDiagnostics").errors==0,"tire regression reports no runtime error"));
        this=finish(this,());
    } catch(error) {this=finish(this,error);}
}
