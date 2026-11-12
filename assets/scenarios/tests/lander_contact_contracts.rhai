// Verdicts belong to the authored test, executed by the production runtime.
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
            variables:["cutoff","settled","throttle","torque"],max_points:100});
        let complete=result.times!=() && result.times.len()==3
            && !result.downsampled && result.missing_variables.is_empty();
        checks.push(t_true(complete,run.name+" complete trajectory"));
        if !complete {continue;}
        for spec in [["cutoff",run.cutoff],["settled",run.settled],
                     ["throttle",run.throttle]] {
            let good=result.series.contains(spec[0]);
            if good {
                for value in result.series[spec[0]] {
                    good=good && value.is_finite() && abs(value-spec[1])<1.0e-8;
                }
            }
            checks.push(t_true(good,run.name+" "+spec[0]+" expected phase"));
        }
        let damping=true;
        for value in result.series["torque"] {
            damping=damping && value.is_finite();
            if run.name=="SoftContact" || run.name=="AcceptedCutoff" {
                damping=damping && value<0.0;
            }
            if run.name=="AcceptedHandoff" {damping=damping && abs(value)<1.0e-8;}
        }
        checks.push(t_true(damping,run.name+" retains damping until handoff"));
    }
    this.done=true;report_verdict(checks,"LANDER CONTACT CONTRACTS","LANDER_CONTACT_CONTRACTS");
}
