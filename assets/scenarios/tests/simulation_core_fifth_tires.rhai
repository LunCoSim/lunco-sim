// The driver authors an ordinary raycast skid rover and deck through typed
// document operations. Dynamic/comoving cases include the deck in body_paths;
// stationary uses a static deck. Comoving starts both native bodies at +2 m/s Z
// with zero wheel spin, drive and damping. No authored USD is saved.
fn simulation_dependencies(me,ctx) {
    let reads=[];let writes=[];
    for path in ctx.body_paths {reads.push(find_path(path));}
    reads.push(find_path(ctx.deck_path));
    for corner in ["FL","FR","RL","RR"] {
        let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
        reads.push(wheel);writes.push(wheel);
    }
    #{modelica_entities:[],entity_reads:reads,entity_writes:writes,
      query_reads:["GetPort","QueryPhysicsState","GetReadiness","RuntimeDiagnostics"],required_inputs:[]}
}
fn sample(entity,name) {
    let value=query("GetPort",#{api_id:entity,name:name});
    if value.value==() {throw("required wheel sample unavailable: "+name+" "+value);}
    value.value
}
fn momentum(paths) {
    let total=0.0;
    for path in paths {
        let body=find_path(path);let state=query("QueryPhysicsState",#{id:body});
        if state.body_mode!="dynamic" {throw("momentum member is not dynamic: "+path);}
        total+=sample(body,"mass")*state.linear_velocity_mps[2];
    }
    total
}
fn finish(state,error) {
    if error!=() {state.checks.push(error.to_string());}
    state.done=true;
    report_verdict(state.checks,"SIMULATION CORE FIFTH TIRES "+state.mode,
        "SIMULATION_CORE_FIFTH_TIRES");state
}
fn on_start(me,ctx) {
    this=#{done:false,frames:0,phase:0,mark:0,mode:ctx.mode,checks:[],max_force:0.0,
        max_spin:0.0,last_sample:()};
    if ctx.mode!="dynamic" && ctx.mode!="comoving" && ctx.mode!="stationary" {
        this=finish(this,"mode must be dynamic, comoving or stationary");
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1800 {this=finish(this,"tire observer timed out: "+this.last_sample);return;}
    try {
        let ready=query("GetReadiness");if ready.faulted {throw("raycast fixture faulted: "+ready);}
        if !ready.ready || ready.world_hold {return;}
        let rover=find_path(ctx.rover_path);let deck=find_path(ctx.deck_path);
        let state=query("QueryPhysicsState",#{id:rover});
        let support=query("QueryPhysicsState",#{id:deck});
        let loaded=0;let force=0.0;
        for contact in state.wheel_contacts {
            if contact.contact_valid && contact.normal_force_n>0.0 {
                loaded+=1;let f=contact.tire_force_n;
                force+=abs(f[0])+abs(f[2]);
            }
        }
        let spin=0.0;
        for corner in ["FL","FR","RL","RR"] {
            let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
            spin+=abs(sample(wheel,"shaft_speed"));
        }
        this.last_sample=#{loaded:loaded,force:force,spin:spin,
            rover:state.linear_velocity_mps,deck:support.linear_velocity_mps};
        if this.phase==0 {
            if loaded!=4 {return;}
            this.checks.push(t_true(true,"four native raycast contacts carry load"));
            if this.mode=="comoving" {
                this.checks.push(t_true(abs(state.linear_velocity_mps[2]-2.0)<ctx.velocity_tolerance
                    && abs(support.linear_velocity_mps[2]-2.0)<ctx.velocity_tolerance,
                    "co-moving fixture has the prescribed shared translation"));
            } else {
                this.checks.push(t_true(abs(state.linear_velocity_mps[2])<ctx.velocity_tolerance,
                    "drive fixture begins without tangential chassis motion"));
            }
            this.p0=momentum(ctx.body_paths);this.v0=state.linear_velocity_mps[2];
            this.deck_v0=support.linear_velocity_mps[2];this.mark=sim_tick();this.phase=1;
            for corner in ["FL","FR","RL","RR"] {
                let wheel=find_path(ctx.rover_path+"/Suspension_"+corner+"/Wheel_"+corner);
                let drive=if this.mode=="comoving" {0.0} else {20.0};
                let result=cmd("SetPorts",#{target:wheel,writes:[["drive",drive]]});
                this.checks.push(t_true(result.ok==true,"wheel command admitted for "+corner));
            }
            return;
        }
        this.max_force=max(this.max_force,force);this.max_spin=max(this.max_spin,spin);
        if sim_tick()-this.mark<60 {return;}
        let p1=momentum(ctx.body_paths);
        if this.mode=="comoving" {
            this.checks.push(t_true(this.max_force<ctx.force_tolerance,
                "shared support translation does not create tire slip force: "+this.max_force));
            this.checks.push(t_true(this.max_spin<ctx.spin_tolerance,
                "co-moving stationary wheels do not acquire artificial spin: "+this.max_spin));
            this.checks.push(t_true(abs(p1-this.p0)<ctx.momentum_tolerance,
                "co-moving contact conserves total tangential momentum"));
        } else if this.mode=="dynamic" {
            this.checks.push(t_true(abs(support.linear_velocity_mps[2]-this.deck_v0)>1.0e-5,
                "dynamic support receives the opposite tire reaction"));
            this.checks.push(t_true(abs(state.linear_velocity_mps[2]-this.v0)>1.0e-5,
                "driven raycast chassis responds"));
            this.checks.push(t_true(abs(p1-this.p0)<ctx.momentum_tolerance,
                "raycast pair conserves tangential momentum: "+this.p0+" -> "+p1));
        } else {
            this.checks.push(t_true(support.body_mode=="static" && abs(support.linear_velocity_mps[2])<1.0e-12,
                "static support retains its prescribed motion"));
            this.checks.push(t_true(abs(state.linear_velocity_mps[2]-this.v0)>1.0e-5,
                "stationary-support drive control remains effective"));
            this.checks.push(t_true(this.max_force>0.0,"stationary support transmits tire force"));
        }
        this.checks.push(t_true(query("RuntimeDiagnostics").errors==0,"contact regression remains healthy"));
        emit("SIMULATION_CORE_FIFTH_TIRE_EVIDENCE",#{mode:this.mode,initial_momentum:this.p0,
            final_momentum:p1,max_force:this.max_force,max_spin:this.max_spin,sample:this.last_sample});
        this=finish(this,());
    } catch(error) {this=finish(this,error);}
}
