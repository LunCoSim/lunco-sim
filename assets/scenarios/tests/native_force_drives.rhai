// Dimensional native motors, tested against the implicit Euler coordinate law.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    let bodies=[];for name in ["Finite","Computed","Damping","ZeroCap","ZeroGains","Angular","Stop","Damped","KinematicDamped","KinematicSpin"] {
        bodies.push(find_path("/World/"+name));
        bodies.push(find_path("/World/"+name+"Host"));bodies.push(find_path("/World/"+name+"Bob"));
    }
    #{modelica_entities:[],entity_reads:bodies,entity_writes:[find_path("/World/Finite")],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn measured(joint,angular) {
    if angular {return [get(joint,"angle"),get(joint,"angular_velocity")];}
    [get(joint,"displacement"),get(joint,"velocity")]
}
fn on_start(me,ctx) {
    let finite=find_path("/World/Finite");
    this.omitted_unlimited=joint_drive(finite).max_force.is_infinite();
    this.setter_unlimited=set_joint_drive(finite,1000.0,100.0,joint_drive(find_path("/World/Computed")).max_force);
    this.setter_rejected_negative=!set_joint_drive(finite,1000.0,100.0,-1.0);
    this.done=false;this.previous=[];this.cases=[];
    this.max_momentum_error=0.0;this.max_kinematic_error=0.0;this.max_damping_error=0.0;this.max_damping_force_error=0.0;this.max_damping_torque_error=0.0;this.previous_damping=[];
    this.mechanics_initialized=false;
}
// Computed mass and inertia belong to native preparation. Establish the
// analytical baseline from the first completed step, before comparing later
// steps; tick-zero scenario initialization precedes that native preparation.
fn initialize_mechanics() {
    let host=query("QueryPhysicsState",#{id:find_path("/World/DampedHost")});
    let bob=query("QueryPhysicsState",#{id:find_path("/World/DampedBob")});
    this.damping_momentum=host.inertia_principal_kgm2[0]*host.angular_velocity_radps[1]+bob.inertia_principal_kgm2[0]*bob.angular_velocity_radps[1];
    // Native shape mass properties originate in f32. Bound conservation by
    // their relative precision while retaining an absolute floor near zero.
    this.momentum_tolerance=max(1.0e-10,abs(this.damping_momentum)*1.1920928955078125e-7);
    this.max_position_error=0.0;this.max_velocity_error=0.0;this.max_force_error=0.0;this.samples=0;
    for name in ["Finite","Computed","Damping","Angular"] {
        let host=query("QueryPhysicsState",#{id:find_path("/World/"+name+"Host")});
        let bob=query("QueryPhysicsState",#{id:find_path("/World/"+name+"Bob")});
        let angular=name=="Angular";
        if !host.mass_kg.is_finite() || !bob.mass_kg.is_finite()
                || host.mass_kg<=0.0 || bob.mass_kg<=0.0
                || (angular && (host.inertia_principal_kgm2[0]<=0.0
                    || bob.inertia_principal_kgm2[0]<=0.0)) {
            throw("native force-drive baseline requires prepared positive mass and inertia");
        }
        let w=if angular {1.0/host.inertia_principal_kgm2[0]+1.0/bob.inertia_principal_kgm2[0]}
              else {if name=="Computed" {1.0/bob.mass_kg} else {1.0/host.mass_kg+1.0/bob.mass_kg}};
        this.cases.push(#{name:name,id:find_path("/World/"+name),angular:angular,w:w,
                         k:if name=="Damping" {0.0} else {if angular {10.0*57.29577951308232} else {1000.0}},
                         c:if angular {2.0*57.29577951308232} else {100.0},
                         target:if name=="Damping" {0.0} else {if angular {10.0*0.017453292519943295} else {0.1}}});
    }
}
fn on_tick(me,ctx) {
    if this.done {return;}
    if !this.mechanics_initialized {
        this.initialize_mechanics();
        this.mechanics_initialized=true;
    }
    let host=query("QueryPhysicsState",#{id:find_path("/World/DampedHost")});
    let bob=query("QueryPhysicsState",#{id:find_path("/World/DampedBob")});
    let momentum=host.inertia_principal_kgm2[0]*host.angular_velocity_radps[1]+bob.inertia_principal_kgm2[0]*bob.angular_velocity_radps[1];
    this.max_momentum_error=max(this.max_momentum_error,(momentum-this.damping_momentum).abs());
    let linear_host=query("QueryPhysicsState",#{id:find_path("/World/KinematicDampedHost")});
    let angular_host=query("QueryPhysicsState",#{id:find_path("/World/KinematicSpinHost")});
    this.max_kinematic_error=max(this.max_kinematic_error,linear_host.linear_velocity_mps[0].abs());
    this.max_kinematic_error=max(this.max_kinematic_error,angular_host.angular_velocity_radps[1].abs());
    let clock=clock_snapshot();
    let damped=[get(find_path("/World/KinematicDamped"),"velocity"),get(find_path("/World/KinematicSpin"),"angular_velocity")];
    if this.previous_damping.len()!=0 {
        let factor=1.0;
        for substep in 0..physics_substeps() {factor*=(-20.0*clock.physics_substep_dt_s).exp();}
        for i in 0..damped.len() {this.max_damping_error=max(this.max_damping_error,(damped[i]-this.previous_damping[i]*factor).abs());}
        let bob=query("QueryPhysicsState",#{id:find_path("/World/KinematicDampedBob")});
        let expected=(damped[0]-this.previous_damping[0])*bob.mass_kg/clock.physics_dt_s;
        this.max_damping_force_error=max(this.max_damping_force_error,
            (get(find_path("/World/KinematicDamped"),"force")-expected).abs());
        let spin=query("QueryPhysicsState",#{id:find_path("/World/KinematicSpinBob")});
        let expected_torque=(damped[1]-this.previous_damping[1])*spin.inertia_principal_kgm2[1]/clock.physics_dt_s;
        this.max_damping_torque_error=max(this.max_damping_torque_error,
            (get(find_path("/World/KinematicSpin"),"torque")-expected_torque).abs());
    }
    this.previous_damping=damped;
    let current=[];
    for i in 0..this.cases.len() {
        let drive=this.cases[i];let state=measured(drive.id,drive.angular);current.push(state);
        if this.previous.len()!=0 {
            let x=this.previous[i][0];let v=this.previous[i][1];let initial_v=v;
            let h=clock.physics_substep_dt_s;
            if h==() || h<=0.0 {throw("the completed native substep clock is unavailable");}
            for substep in 0..physics_substeps() {
                x+=h*v;
                let change=(drive.k*(drive.target-x)-drive.c*v)*drive.w*h/
                           (1.0+drive.w*drive.c*h+drive.w*drive.k*h*h);
                v+=change;x+=h*change;
            }
            this.max_position_error=max(this.max_position_error,(state[0]-x).abs());
            this.max_velocity_error=max(this.max_velocity_error,(state[1]-v).abs());
            let expected_effort=(v-initial_v)/(drive.w*clock.physics_dt_s);
            let effort_port=if drive.angular {"torque"} else {"force"};
            this.max_force_error=max(this.max_force_error,(get(drive.id,effort_port)-expected_effort).abs());
        }
    }
    this.previous=current;this.samples+=1;
    if this.samples<20 {return;}
    this.done=true;
    let computed=query("QueryPhysicsState",#{id:find_path("/World/ComputedBob")});
    report_verdict([
        t_true(this.omitted_unlimited,"an omitted USD maxForce retains the native unlimited default"),
        t_true(this.setter_unlimited && joint_drive(find_path("/World/Finite")).max_force.is_infinite(),"the live drive setter retains the unlimited-force contract"),
        t_true(this.setter_rejected_negative,"the live drive setter rejects a negative force cap without mutation"),
        t_true(joint_drive(find_path("/World/Computed")).max_force.is_infinite(),"explicit positive infinity retains unlimited drive effort"),
        t_true(this.max_damping_force_error<1e-9,"the completed reaction includes passive linear damping impulse"),
        t_true(this.max_damping_torque_error<1e-9,"the completed reaction includes passive angular damping impulse"),
        t_true(this.max_momentum_error<this.momentum_tolerance,"passive joint damping conserves unequal-inertia momentum"),
        t_true(this.max_kinematic_error<1e-12,"passive joint damping preserves kinematic motion authority"),
        t_true(this.max_damping_error<1e-10,"passive joint damping integrates its decay rate against infinite-inertia endpoints"),
        t_true(get(find_path("/World/Damped"),"angular_velocity").abs()<0.01,"angular damping dissipates relative spin"),
        t_true((get(find_path("/World/Stop"),"displacement")-0.02).abs()<1e-8 && get(find_path("/World/Stop"),"force").abs()<1e-5,"opposing motor and travel-stop reactions cancel in the net force port"),
        t_true(this.samples>=20,"twenty completed physics samples were observed"),
        t_true(this.max_position_error<1e-8,"force motors preserve the implicit dimensional position law"),
        t_true(this.max_velocity_error<1e-7,"force motors use both native endpoint inertias"),
        t_true(this.max_force_error<1e-5,"force and torque ports report the completed native reaction"),
        t_true(computed.authored_mass_kg==() && computed.mass_kg>0.0,"computed mass uses the same native force law"),
        t_true(get(find_path("/World/Finite"),"displacement")>0.01,"the finite-mass spring actually moves"),
        t_true(get(find_path("/World/Damping"),"velocity")<0.1,"pure damping dissipates relative motion"),
        t_true(get(find_path("/World/ZeroCap"),"displacement").abs()<1e-12 && get(find_path("/World/ZeroCap"),"force").abs()<1e-12,"a zero force cap prevents actuation"),
        t_true(get(find_path("/World/ZeroGains"),"displacement").abs()<1e-12,"omitted USD gains retain their zero semantic defaults"),
    ],"Native force drives","NATIVE_FORCE_DRIVES");
    print("NATIVE FORCE DRIVES "+#{maximum_momentum_error:this.max_momentum_error,maximum_kinematic_error:this.max_kinematic_error,maximum_position_error:this.max_position_error,maximum_velocity_error:this.max_velocity_error,maximum_effort_error:this.max_force_error,maximum_damping_force_error_N:this.max_damping_force_error,maximum_damping_torque_error_Nm:this.max_damping_torque_error});
}
