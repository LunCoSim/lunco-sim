// Verdicts for production Modelica solves supplied by the API lifecycle driver.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn near(values, expected, tolerance) {
    for value in values {if !value.is_finite() || abs(value-expected)>tolerance {return false;}}
    true
}
fn on_start(me, params) {this.done=false;}
fn on_tick(me, params) {
    if this.done {return;}
    let status=query("RunStatus",#{experiment_id:params.run});
    if status.state=="failed" || status.state=="cancelled" {
        fail_fast(status.to_string(),"MECHANICAL LOSSES","MECHANICAL_LOSSES");this.done=true;return;
    }
    if status.state!="done" {return;}
    let specs=[["drive",2.0,10.0,-8.0],["backdrive",2.5,-10.0,12.5],
        ["reverse",2.0,10.0,-8.0],["reverse_backdrive",2.5,-10.0,12.5],
        ["neutral",0.0,0.0,0.0],["stopped",0.0,0.0,0.0],
        ["ideal",0.0,10.0,-10.0],["negative_ratio",2.5,-10.0,12.5]];
    let names=["power_balance_w","thermal_balance_j","thermal_rise_k","reversal_heat_w","contact_power_balance_w","contact_energy_balance_j","tire_rise_k","ground_rise_k"];
    for spec in specs {for leaf in ["heat","power_a_w","power_b_w"] {names.push(spec[0]+".gear."+leaf);}}
    for name in ["bearing_forward","bearing_reverse","bearing_ideal"] {
        names.push(name+".bearing.heat");names.push(name+".bearing.resisting_torque_nm");
    }
    let result=query("GetExperimentResult",#{experiment_id:params.run,variables:names,max_points:64});
    if result.series==() || result.times==() || result.times.is_empty() {
        fail_fast("missing mechanical trajectory","MECHANICAL LOSSES","MECHANICAL_LOSSES");this.done=true;return;
    }
    let checks=[];let s=result.series;
    let complete=true;
    for name in names {
        complete=complete && s.contains(name) && s[name].len()==result.times.len();
        if s.contains(name) {for value in s[name] {complete=complete && value.is_finite();}}
    }
    if !complete {fail_fast("incomplete mechanical trajectory","MECHANICAL LOSSES","MECHANICAL_LOSSES");this.done=true;return;}
    checks.push(t_true(complete,"all requested trajectories are complete and finite"));
    for spec in specs {
        checks.push(t_true(near(s[spec[0]+".gear.heat"],spec[1],1.0e-7),spec[0]+" passive mesh loss"));
        checks.push(t_true(near(s[spec[0]+".gear.power_a_w"],spec[2],1.0e-7),spec[0]+" fast-side power"));
        checks.push(t_true(near(s[spec[0]+".gear.power_b_w"],spec[3],1.0e-7),spec[0]+" slow-side power"));
    }
    for spec in [["bearing_forward",1.8,0.9],["bearing_reverse",1.8,-0.9],["bearing_ideal",0.0,0.0]] {
        checks.push(t_true(near(s[spec[0]+".bearing.heat"],spec[1],1.0e-7),spec[0]+" nonnegative viscous heat"));
        checks.push(t_true(near(s[spec[0]+".bearing.resisting_torque_nm"],spec[2],1.0e-7),spec[0]+" signed drag torque"));
    }
    checks.push(t_true(near(s.power_balance_w,0.0,1.0e-6),"electrical power equals delivered shaft work plus copper, gear and bearing heat"));
    checks.push(t_true(near(s.thermal_balance_j,0.0,1.0e-6),"thermal mass gains the integrated dissipated energy"));
    checks.push(t_true(s.thermal_rise_k[s.thermal_rise_k.len()-1]>0.0,"losses actually heat the thermal mass"));
    let passive=true;let both_signs=false;
    for v in s.reversal_heat_w {passive=passive && v>=-1.0e-8;}
    both_signs=s.reversal_heat_w[0]>0.0 && s.reversal_heat_w[s.reversal_heat_w.len()-1]>0.0;
    checks.push(t_true(passive && both_signs,"reversing power flow remains passive through zero speed"));
    let diagnostics=["requires 0 < eta <= 1","requires 0 < eta <= 1","requires a nonzero ratio",
        "output torque exceeds its rating","requires nonnegative viscous damping","output torque exceeds its rating"];
    checks.push(t_true(near(s.contact_power_balance_w,0.0,1.0e-7),"contact interface conserves wheel and terrain heat flow"));
    checks.push(t_true(near(s.contact_energy_balance_j,0.0,1.0e-6),"wheel plus terrain stored heat equals friction input"));
    checks.push(t_true(s.tire_rise_k[s.tire_rise_k.len()-1]>0.0,"contact friction heats the wheel mass"));
    checks.push(t_true(s.ground_rise_k[s.ground_rise_k.len()-1]>0.0,"contact friction heats the terrain mass"));
    let isolated=true;
    for i in 0..result.times.len() {
        if result.times[i]<0.99 {isolated=isolated && abs(s.ground_rise_k[i])<1.0e-10;}
    }
    checks.push(t_true(isolated,"opening the contact conductance prevents airborne ground heating"));
    for index in 0..params.invalid_runs.len() {
        let run=params.invalid_runs[index];
        let failed=query("RunStatus",#{experiment_id:run});
        if failed.state!="failed" && failed.state!="done" && failed.state!="cancelled" {return;}
        checks.push(t_true(failed.state=="failed" && failed.error!=() && failed.error.contains(diagnostics[index])
            && (index!=5 || !failed.error.contains("at t=0.000000000")),"invalid mechanical operating point fails visibly"));
    }
    this.done=true;report_verdict(checks,"MECHANICAL LOSSES","MECHANICAL_LOSSES");
}
