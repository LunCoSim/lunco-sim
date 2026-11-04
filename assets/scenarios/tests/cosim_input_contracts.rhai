// API driver admits controller mass=7 before this Twin/Simulation observer runs.
// Fixture: /World/Target has an unwired mass input; /World/ForceBody is a free
// dynamic unit-mass body with zero damping and no force-input connections.
fn simulation_dependencies(me,ctx) {
    let entities=[find_path("/World/Target"),find_path("/World/ForceBody")];
    #{modelica_entities:[],entity_reads:entities,entity_writes:entities,
      query_reads:["GetPort","QueryPhysicsState","GetReadiness"],required_inputs:[]}
}
fn port(entity,name) {
    let result=query("GetPort",#{api_id:entity,name:name});
    if result==() || result.ok==false || result.value==() {throw("port sample unavailable: "+name+" "+result);}
    result.value
}
fn speed_x(entity) { query("QueryPhysicsState",#{id:entity}).linear_velocity_mps[0] }
fn on_start(me,ctx) {
    this.phase="control";this.ticks=0;this.done=false;this.checks=[];
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.ticks+=1;
    if this.ticks>1200 {
        this.checks.push("input contract observer timed out in "+this.phase);
        this.done=true;report_verdict(this.checks,"COSIM INPUT CONTRACTS","COSIM_INPUT_CONTRACTS");return;
    }
    try {
        let target=find_path("/World/Target");let body=find_path("/World/ForceBody");
        emit("COSIM_INPUT_SAMPLE",#{phase:this.phase,tick:sim_tick(),speed:speed_x(body),
            physics_elapsed:clock_snapshot().physics_elapsed_s});
        if this.phase=="control" {
            if abs(port(target,"mass")-7.0)>0.00001 {return;}
            this.checks.push(t_true(true,"external controller input was admitted"));
            this.checks.push(t_true(cmd("SetPorts",#{target:target,writes:[["mass",8.0]]}).ok==true,
                "the authored program setpoint was rejected"));
            this.checks.push(t_true(abs(port(target,"mass")-7.0)<0.00001,
                "a program write replaced a controller-held unwired mass"));
            this.v0=speed_x(body);this.mark=sim_tick();
            this.checks.push(t_true(cmd("SetPorts",#{target:body,writes:[["force_x",12.0]]}).ok==true,
                "the unwired force level was rejected"));
            let rejected=cmd("SetPortsBatch",#{batches:[
                #{target:target,writes:[["mass",9.0]]},
                #{target:body,writes:[["mass",2.0],["force_x",1.0e300]]}]});
            this.checks.push(t_true(rejected.ok==false,"out-of-range force batch was accepted"));
            this.checks.push(t_true(abs(port(target,"mass")-7.0)<0.00001,
                "a rejected batch changed the controller-held target"));
            this.checks.push(t_true(abs(port(body,"mass")-1.0)<0.00001,
                "a rejected batch partially changed native mass"));
            this.checks.push(t_true(port(body,"force_x")==12.0,
                "a rejected batch changed the already-admitted force command"));
            this.phase="arming";return;
        }
        if this.phase=="arming" {
            // Authored commands enter the next fixed input boundary. Establish
            // the native baseline after that admission, then measure steady levels.
            if sim_tick()-this.mark<2 {return;}
            this.v0=speed_x(body);this.mark=sim_tick();this.phase="force";return;
        }
        if this.phase=="force" {
            if sim_tick()-this.mark<4 {return;}
            let elapsed=(sim_tick()-this.mark).to_float()*dt();
            let change=speed_x(body)-this.v0;
            this.checks.push(t_true(abs(change-12.0*elapsed)<0.0001,
                "unwired force did not persist: dv="+change+" expected="+(12.0*elapsed)));
            this.checks.push(t_true(abs(port(target,"mass")-7.0)<0.00001,
                "the retained controller setpoint changed during propagation"));
            this.checks.push(t_true(cmd("ReleasePort",#{target:body,name:"force_x"}).ok==true,
                "force-level release was rejected"));
            this.checks.push(t_true(cmd("ReleasePort",#{target:target,name:"mass"}).ok==true,
                "mass hold release was rejected"));
            this.checks.push(t_true(abs(port(target,"mass")-7.0)<0.00001,
                "release rewrote the endpoint value"));
            this.mark=sim_tick();this.phase="releasing";return;
        }
        if this.phase=="releasing" {
            // The already admitted physical interval completes before the
            // release boundary; subsequent intervals must coast.
            if sim_tick()-this.mark<2 {return;}
            this.released_speed=speed_x(body);this.mark=sim_tick();this.phase="released";return;
        }
        if sim_tick()-this.mark<4 {return;}
        this.checks.push(t_true(abs(speed_x(body)-this.released_speed)<0.0001,
            "released force continued to accelerate the body"));
        this.checks.push(t_true(cmd("SetPorts",#{target:target,writes:[["mass",8.0]]}).ok==true
            && abs(port(target,"mass")-8.0)<0.00001,
            "an unheld unwired input did not accept its authored program value"));
        this.checks.push(t_true(query("GetReadiness").faulted==false,
            "a rejected user input faulted the simulation"));
        this.done=true;report_verdict(this.checks,"COSIM INPUT CONTRACTS","COSIM_INPUT_CONTRACTS");
    } catch(error) {
        this.checks.push("input contract observer failed: "+error);
        this.done=true;report_verdict(this.checks,"COSIM INPUT CONTRACTS","COSIM_INPUT_CONTRACTS");
    }
}
