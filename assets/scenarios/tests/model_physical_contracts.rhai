// Production Modelica energy, connector and controller acceptance.
// The API driver supplies the exact two production experiment IDs.

fn simulation_dependencies(me, params) {
    #{modelica_entities:[], entity_reads:[], entity_writes:[],
      query_reads:["CompileStatus","GetDocumentSource","RunStatus","GetExperimentResult"],
      required_inputs:[]}
}
fn all_near(values, expected, tolerance) {
    for value in values {if abs(value-expected)>tolerance {return false;}}
    true
}
fn bounded(values, limit) {
    for value in values {if abs(value)>limit+1.0e-6 {return false;}}
    true
}
fn last(values) {values[values.len()-1]}
fn output_names(index) {
    if index==0 {return ["forward_balance_w","reverse_balance_w","regeneration_balance_w","braking_balance_w",
        "forward_torque_nm","reverse_torque_nm","regeneration_power_w","braking_power_w",
        "upper_command","lower_command","reversal_command","upper_integral","lower_integral","reversal_integral",
        "meter_voltage_v","meter_current_error_a","meter_power_w","thermal_probe_flow_w",
        "body_temperature_k","cryogenic_temperature_k","boiloff_rate_kgs","cryogenic_mass_kg",
        "unpowered_heater_heat_w","pump_flow_balance_kgs","pump_work_balance_w","pump_mass_balance_kg","pump_chamber_thrust_n"];}
    let names=[];
    for i in 1..7 {names.push("orthogonal.command["+i+"]");}
    for i in 1..4 {names.push("coupled.command["+i+"]");}
    names.push("powerless.command[1]");names
}
fn on_start(me, params) {
    this.done=false;this.results=[(),()];this.runs=[params.contract_run,params.allocator_run];
}
fn on_tick(me, params) {
    if this.done {return;}
    for i in 0..2 {
        if this.results[i]!=() {continue;}
        let status=query("RunStatus",#{experiment_id:this.runs[i]});
        if status.state=="failed" || status.state=="cancelled" {
            fail_fast(status.to_string(),"MODEL PHYSICAL CONTRACTS","MODEL_PHYSICAL_CONTRACTS");this.done=true;return;
        }
        if status.state!="done" {return;}
        let result=query("GetExperimentResult",#{experiment_id:this.runs[i],variables:output_names(i),max_points:64});
        if result==() || result.series==() || result.times==() || result.times.is_empty() || result.times.len()>64 {
            fail_fast("completed solve has no trajectory","MODEL PHYSICAL CONTRACTS","MODEL_PHYSICAL_CONTRACTS");this.done=true;return;
        }
        for name in result.series.keys() {
            let values=result.series[name];
            let valid=values.len()==result.times.len();
            for value in values {valid=valid && value.is_finite();}
            if !valid {fail_fast("incomplete/non-finite trajectory: "+name,"MODEL PHYSICAL CONTRACTS","MODEL_PHYSICAL_CONTRACTS");this.done=true;return;}
        }
        this.results[i]=result;
    }
    if this.results[0]==() || this.results[1]==() {return;}
    this.done=true;
    let s=this.results[0].series;let a=this.results[1].series;let checks=[];
    for mode in ["forward","reverse","regeneration","braking"] {
        checks.push(t_true(all_near(s[mode+"_balance_w"],0.0,1.0e-6),mode+" motor conserves electrical, shaft and thermal power"));
    }
    checks.push(t_true(all_near(s.forward_torque_nm,0.45,1.0e-8),"forward nameplate torque-speed curve"));
    checks.push(t_true(all_near(s.reverse_torque_nm,-0.45,1.0e-8),"reverse nameplate torque-speed curve"));
    checks.push(t_true(last(s.regeneration_power_w)<0.0,"overspeed returns electrical power"));
    checks.push(t_true(all_near(s.braking_power_w,0.0,1.0e-8),"zero duty braking needs no bus power"));
    for axis in ["upper","lower","reversal"] {
        checks.push(t_true(bounded(s[axis+"_command"],0.25),axis+" command honors integral limit"));
        let same=true;
        for i in 0..s[axis+"_command"].len() {same=same && abs(s[axis+"_command"][i]-s[axis+"_integral"][i])<1.0e-8;}
        checks.push(t_true(same,axis+" uses its reported bounded integral"));
    }
    checks.push(t_true(abs(last(s.upper_command)-0.25)<1.0e-6,"positive integral reaches its bound"));
    checks.push(t_true(abs(last(s.lower_command)+0.25)<1.0e-6,"negative integral reaches its bound"));
    checks.push(t_true(abs(last(s.reversal_command)+0.25)<1.0e-6,"reversal unwinds without hidden accumulated error"));
    for spec in [["meter_voltage_v",28.0],["meter_current_error_a",0.0],
                 ["meter_power_w",0.0],["thermal_probe_flow_w",0.0]] {
        checks.push(t_true(all_near(s[spec[0]],spec[1],1.0e-8),spec[0]+" connector conservation"));
    }
    checks.push(t_true(abs(last(s.body_temperature_k)-293.225)<1.0e-6,"probe does not alter heat-capacity balance"));
    checks.push(t_true(all_near(s.cryogenic_temperature_k,90.0,1.0e-8),"phase-change temperature is authored"));
    checks.push(t_true(all_near(s.boiloff_rate_kgs,0.001,1.0e-8),"latent heat closes boiloff energy balance"));
    checks.push(t_true(abs(last(s.cryogenic_mass_kg)-9.967)<1.0e-6,"flow and boiloff close tank mass balance"));
    checks.push(t_true(all_near(s.unpowered_heater_heat_w,0.0,1.0e-10),"an unpowered heater cannot create heat"));
    for spec in [["pump_flow_balance_kgs",1.0e-8],["pump_work_balance_w",1.0e-6],["pump_mass_balance_kg",1.0e-7]] {
        checks.push(t_true(all_near(s[spec[0]],0.0,spec[1]),spec[0]+" conservative connected pump/chamber network"));
    }
    checks.push(t_true(last(s.pump_chamber_thrust_n)>0.0,"connected tank/pump/chamber network fires"));
    let expected=[0.25,-0.4,1.0,-1.0,0.0,0.7];
    for i in 1..7 {checks.push(t_true(all_near(a["orthogonal.command["+i+"]"],expected[i-1],1.0e-6),"orthogonal bounded allocation "+i));}
    for i in 1..4 {checks.push(t_true(bounded(a["coupled.command["+i+"]"],1.0) && last(a["coupled.command["+i+"]"])>=0.0,"coupled actuator bound "+i));}
    checks.push(t_true(abs(last(a["coupled.command[1]"])+last(a["coupled.command[2]"])-1.0)<1.0e-6,"coupled first wrench row"));
    checks.push(t_true(abs(last(a["coupled.command[2]"])+last(a["coupled.command[3]"])-1.0)<1.0e-6,"coupled second wrench row"));
    checks.push(t_true(all_near(a["powerless.command[1]"],0.25,1.0e-8),"zero-authority actuator stays at lower bound"));
    let small=query("GetExperimentResult",#{experiment_id:this.runs[0],
        variables:["forward_balance_w","upper_command"],max_points:6});
    checks.push(t_true(small.times.len()<=6 && abs(last(small.times)-3.0)<1.0e-8,
        "small trajectory cap retains the horizon endpoint"));
    checks.push(t_true(small.series.forward_balance_w.len()==small.times.len()
        && all_near(small.series.forward_balance_w,0.0,1.0e-6),
        "constant trajectory columns retain the final timestamp row"));
    report_verdict(checks,"MODEL PHYSICAL CONTRACTS","MODEL_PHYSICAL_CONTRACTS");
}
