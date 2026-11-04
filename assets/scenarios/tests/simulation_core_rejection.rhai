// @timing presentation
// A terminal source failure holds physical time while the application stays available.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.frames=0;this.done=false;this.before=clock_snapshot();
    this.expected_kind=ctx.expected_kind;this.detail=ctx.detail;
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    let ready=query("GetReadiness");
    if !ready.faulted && this.frames<300 {return;}
    if this.frames<3 {return;}
    let now=clock_snapshot();
    this.done=true;
    report_verdict([
        t_true(ready.application_available==true,"the application remains available after invalid physical source"),
        t_true(ready.faulted && ready.fault.kind==this.expected_kind,
            "source failure must reach its runtime owner: "+ready),
        t_true(ready.faulted && ready.fault.detail.contains(this.detail),
            "the source failure must identify the invalid authored value: "+ready),
        t_true(now.sim_tick==this.before.sim_tick && now.physics_elapsed_s==this.before.physics_elapsed_s,
            "invalid physical source must not advance master or native integration"),
        t_true(ready.faulted && ready.fault.subject!="" && ready.fault.detail!="",
            "the retained failure must identify its subject and cause")
    ],"SIMULATION CORE REJECTION","SIMULATION_CORE_REJECTION");
}
