// Twin/Simulation execution is essential: these writes are authored program
// setpoints. The driver first sets /World/LayeredInput.u=1 via external SetPorts.
fn simulation_dependencies(me,ctx) {
    let target=find_path("/World/LayeredInput");
    #{modelica_entities:[target],entity_reads:[target],entity_writes:[target],
      query_reads:["GetPort","GetReadiness","RuntimeDiagnostics"],required_inputs:[]}
}
fn port(target,name) {
    let value=query("GetPort",#{api_id:target,name:name});
    if value.value==() {throw("live Modelica sample unavailable: "+name+" "+value);}
    value.value
}
fn near(value,expected) {abs(value-expected)<1.0e-12}
fn on_start(me,ctx) {this.done=false;this.frames=0;this.phase=0;this.mark=0;this.checks=[];}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1200 {
        this.done=true;this.checks.push("layered input observer timed out at phase "+this.phase);
        report_verdict(this.checks,"SIMULATION CORE FOURTH SIGNALS","SIMULATION_CORE_FOURTH_SIGNALS");return;
    }
    try {
        let target=find_path("/World/LayeredInput");
        if target<0 {throw("live Modelica participant is missing");}
        if query("GetReadiness").faulted {throw("layered input fixture faulted");}
        if this.phase==0 {
            if !near(port(target,"u"),1.0) || !near(port(target,"y"),1.0) {return;}
            this.checks.push(t_true(true,"controller-held input and solver output start at one"));
            let set=cmd("SetModelInput",#{target_gid:target,name:"u",value:2.0});
            this.checks.push(t_true(set.ok==true,"program Modelica input is admitted"));
            this.checks.push(t_true(near(port(target,"u"),1.0),"program write preserves the effective controller value atomically"));
            this.mark=sim_tick();this.phase=1;return;
        }
        if this.phase==1 {
            if sim_tick()-this.mark<5 {return;}
            this.checks.push(t_true(near(port(target,"u"),1.0),"controller input survives later propagation"));
            this.checks.push(t_true(near(port(target,"y"),1.0),"solver consumes controller priority over program guidance"));
            let rejected=cmd("SetModelInput",#{target_gid:target,name:"missing_input",value:9.0});
            this.checks.push(t_true(rejected.ok==false,"unknown Modelica input rejects visibly"));
            this.checks.push(t_true(near(port(target,"u"),1.0),"rejected program input preserves the effective value"));
            let released=cmd("ReleasePort",#{target:target,name:"u"});
            this.checks.push(t_true(released.ok==true,"explicit input release is admitted"));
            this.checks.push(t_true(near(port(target,"u"),1.0),"release preserves the current endpoint sample"));
            let set=cmd("SetModelInput",#{target_gid:target,name:"u",value:2.0});
            this.checks.push(t_true(set.ok==true && near(port(target,"u"),2.0),"program owns the released input through the same layered writer"));
            this.mark=sim_tick();this.phase=2;return;
        }
        if sim_tick()-this.mark<5 {return;}
        this.checks.push(t_true(near(port(target,"u"),2.0) && near(port(target,"y"),2.0),"program level persists and reaches the solver"));
        this.checks.push(t_true(query("RuntimeDiagnostics").errors==0,"rejected input did not fault the simulation"));
        this.done=true;report_verdict(this.checks,"SIMULATION CORE FOURTH SIGNALS","SIMULATION_CORE_FOURTH_SIGNALS");
    } catch(error) {
        this.done=true;this.checks.push(error.to_string());
        report_verdict(this.checks,"SIMULATION CORE FOURTH SIGNALS","SIMULATION_CORE_FOURTH_SIGNALS");
    }
}
