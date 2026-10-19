// @timing presentation
// Invalid gas parameters fail at the Modelica owner, retaining its last sample.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","GetPort"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.engine=find_path("/World/Engine");this.checks=[];this.frames=0;
}
fn task(me,ctx) {
    seq([
        once(|m| {
            let result=cmd("SetPorts",#{target:this.engine,
                writes:[["thrust_n",1000.0],["propellant_flow_kgs",1000.0/2600.0]]});
            if !result.ok {throw(result.to_string());}
        }),
        wait_until(|m| get(this.engine,"exit_speed_mps")>0.0),
        once(|m| {
            this.checks.push(t_true(get(this.engine,"exit_speed_mps")>0.0,"valid gas first produces a transported jet"));
            let result=cmd("SetPorts",#{target:this.engine,writes:[["gas_gamma",0.9]]});
            this.checks.push(t_true(result.ok,"negative gas stimulus reaches the model input"));
        }),
        wait_until(|m| query("GetReadiness").faulted),
        once(|m| {
            let ready=query("GetReadiness");
            this.checks.push(t_true(ready.fault.kind=="modelica-step-failed",
                "the Modelica owner rejects an invalid gas law"));
            this.checks.push(t_true(ready.fault.detail.contains("ideal-gas gamma"),
                "the diagnostic identifies the violated gas-law constraint"));
            this.stopped=get(this.engine,"transported_distance_m");
        }),
        wait_until(|m| {this.frames+=1;this.frames>8}),
        once(|m| {
            this.checks.push(t_true(query("GetReadiness").faulted
                && get(this.engine,"transported_distance_m")==this.stopped,
                "the failed participant retains its last sample without crashing"));
            report_verdict(this.checks,"PLUME INVALID GAS","PLUME_INVALID_GAS");
        })
    ])
}
