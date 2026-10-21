fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn on_start(me,ctx) {this.done=false;}
fn near_all(values,expected,tolerance) {
    for value in values {if abs(value-expected)>tolerance {return false;}}true
}
fn on_tick(me,ctx) {
    if this.done {return;}
    for run in ctx.runs {
        let status=query("RunStatus",#{experiment_id:run.experiment_id});
        if status.state!="done" && status.state!="failed" && status.state!="cancelled" {return;}
    }
    let checks=[];
    for run in ctx.runs {
        let status=query("RunStatus",#{experiment_id:run.experiment_id});
        if run.label=="InvalidEmissivity" {
            checks.push(t_true(status.state=="failed" && status.error!=()
                && status.error.contains("Radiator requires"),"invalid emissivity rejects at its equation owner"));
            continue;
        }
        if status.state!="done" {fail_fast("radiative exchange experiment failed: "+status,
            "RADIATIVE EXCHANGE","RADIATIVE_EXCHANGE");this.done=true;return;}
        let result=query("GetExperimentResult",#{experiment_id:run.experiment_id,
            variables:["temperature_k","rejection_w","balance_w","exchange_law_w"],max_points:4000});
        let complete=result.times!=() && result.times.len()==25 && !result.downsampled
            && result.missing_variables.is_empty() && result.times[0]==0.0
            && abs(result.times[result.times.len()-1]-120.0)<1.0e-8;
        for variable in ["temperature_k","rejection_w","balance_w","exchange_law_w"] {
            complete=complete && result.series[variable]!=() && result.series[variable].len()==result.times.len();
            if result.series[variable]!=() {for value in result.series[variable] {complete=complete && value.is_finite();}}
        }
        if !complete {fail_fast("complete finite trajectory required: "+run.label,
            "RADIATIVE EXCHANGE","RADIATIVE_EXCHANGE");this.done=true;return;}
        checks.push(t_true(complete,run.label+" retains its complete finite trajectory"));
        checks.push(t_true(near_all(result.series.balance_w,0.0,1.0e-7),run.label+" closes the thermal node"));
        checks.push(t_true(near_all(result.series.exchange_law_w,0.0,1.0e-7),run.label+" follows signed Stefan-Boltzmann exchange"));
        let t=result.series.temperature_k;let q=result.series.rejection_w;let last=t.len()-1;
        let physical=true;
        if run.label=="Heating" {
            for i in 0..t.len() {physical=physical && q[i]<0.0 && t[i]>=250.0 && t[i]<=350.0;}
            physical=physical && t[last]>t[0]+1.0;
        } else if run.label=="Cooling" {
            for i in 0..t.len() {physical=physical && q[i]>0.0 && t[i]>=250.0 && t[i]<=300.0;}
            physical=physical && t[last]<t[0]-1.0;
        } else {physical=near_all(t,300.0,1.0e-8) && near_all(q,0.0,1.0e-8);}
        checks.push(t_true(physical,run.label+" has the correct heat-flow direction and temperature response"));
    }
    report_verdict(checks,"RADIATIVE EXCHANGE","RADIATIVE_EXCHANGE");this.done=true;
}
