// Analytic directions exercise sign, antipodal recovery, tilt and manual gating.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn on_start(me, params) { this.done=false; }
fn on_tick(me, params) {
    if this.done {return;}
    for run in params.runs {
        let status=query("RunStatus",#{experiment_id:run.id});
        if status.state!="done" && status.state!="failed" {return;}
    }
    let checks=[];
    for run in params.runs {
        let status=query("RunStatus",#{experiment_id:run.id});
        checks.push(t_true(status.state=="done",run.name+" compiles and runs"));
        if status.state!="done" {print(status);continue;}
        let result=query("GetExperimentResult",#{experiment_id:run.id,
            variables:["heading","valid","ex","ey","ez"],max_points:100});
        let complete=result.times!=() && result.times.len()==3
            && !result.downsampled && result.missing_variables.is_empty();
        checks.push(t_true(complete,run.name+" complete trajectory"));
        if !complete {continue;}
        for spec in [["heading",run.heading],["valid",run.valid],
            ["ex",run.ex],["ey",run.ey],["ez",0.0]] {
            let good=result.series.contains(spec[0]);
            if good {
                for v in result.series[spec[0]] {
                    good=good && v.is_finite() && abs(v-spec[1])<1.0e-8;
                }
            }
            checks.push(t_true(good,run.name+" "+spec[0]+" analytic reference"));
        }
    }
    this.done=true;report_verdict(checks,"ATTITUDE HEADING","ATTITUDE_HEADING");
}
