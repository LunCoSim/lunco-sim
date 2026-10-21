fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn on_start(me, params) {this.done=false;this.run=params.contract_run;}
fn on_tick(me, params) {
    if this.done {return;}
    let status=query("RunStatus",#{experiment_id:this.run});
    if status.state=="failed" || status.state=="cancelled" {
        this.done=true;fail_fast(status.to_string(),"LANDER POWER CONTRACTS","LANDER_POWER_CONTRACTS");return;
    }
    if status.state!="done" {return;}
    this.done=true;let fields=["throttle","torque_x","torque_y","torque_z"];let names=[];
    for group in ["powered","off","recovering"] {for field in fields {names.push(group+"."+field);}}
    let result=query("GetExperimentResult",#{experiment_id:this.run,variables:names,max_points:128});
    if result==() || result.times==() || result.times.len()<9 || abs(result.times[result.times.len()-1]-0.6)>1.0e-8 {
        fail_fast("expected complete power-interruption trajectory","LANDER POWER CONTRACTS","LANDER_POWER_CONTRACTS");return;
    }
    let checks=[];
    for t in [0.0,0.1,0.19,0.21,0.3,0.39,0.41,0.5,0.6] {
        // Solvers may retain duplicate event rows; select ordinary output
        // samples on both sides rather than assuming a fixed trajectory size.
        let row=-1;
        for i in 0..result.times.len() {if abs(result.times[i]-t)<1.0e-8 {row=i;break;}}
        if row<0 {fail_fast("missing power-phase sample at "+t,"LANDER POWER CONTRACTS","LANDER_POWER_CONTRACTS");return;}
        let available=t<0.2 || t>0.4;
        for field in fields {
            let active=result.series["powered."+field][row];
            let off=result.series["off."+field][row];
            let recovering=result.series["recovering."+field][row];
            checks.push(t_true(active.is_finite() && abs(active)>1.0e-6,"powered "+field+" at "+t));
            checks.push(t_true(off.is_finite() && abs(off)<1.0e-9,"unpowered "+field+" at "+t));
            let expected=if available {active} else {0.0};
            checks.push(t_true(recovering.is_finite() && abs(recovering-expected)<1.0e-9,"recovering "+field+" at "+t));
        }
    }
    report_verdict(checks,"LANDER POWER CONTRACTS","LANDER_POWER_CONTRACTS");
}
