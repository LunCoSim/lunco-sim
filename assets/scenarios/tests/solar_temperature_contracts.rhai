// Synthetic electrothermal witnesses; no flight-cell or cooling-system calibration.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn last(a) { a[a.len()-1] }
fn near_all(a, target, tol) {
    for v in a { if abs(v-target)>tol { return false; } } true
}
fn bounded(a, lo, hi, tol) {
    for v in a { if v<lo-tol || v>hi+tol { return false; } } true
}
fn on_start(me, params) { this.done=false; }
fn on_tick(me, params) {
    if this.done { return; }
    for run in params.runs {
        let s=query("RunStatus",#{experiment_id:run.experiment_id});
        if s.state!="done" && s.state!="failed" && s.state!="cancelled" { return; }
    }
    let checks=[];let results=#{};
    for run in params.runs {
        let s=query("RunStatus",#{experiment_id:run.experiment_id});
        if run.invalid {
            checks.push(t_true(s.state=="failed" && s.error!=()
                && s.error.contains(run.diagnostic),run.label+" rejects invalid physical input"));
            continue;
        }
        if s.state!="done" {
            this.done=true;fail_fast(s.to_string(),"SOLAR TEMPERATURE","SOLAR_TEMPERATURE");return;
        }
        let r=query("GetExperimentResult",#{experiment_id:run.experiment_id,
            variables:run.variables,max_points:4000});
        let complete=r.times!=() && r.times.len()>1 && r.series!=()
            && !r.downsampled && r.missing_variables.is_empty()
            && r.times[0]==0.0 && abs(last(r.times)-run.stop_s)<1.0e-8;
        if complete {
            for name in run.variables {
                complete=complete && r.series.contains(name)
                    && r.series[name].len()==r.times.len();
                if r.series.contains(name) {
                    for v in r.series[name] { complete=complete && v.is_finite(); }
                }
            }
        }
        if !complete {
            this.done=true;fail_fast("Incomplete trajectory: "+run.label,
                "SOLAR TEMPERATURE","SOLAR_TEMPERATURE");return;
        }
        checks.push(t_true(complete,run.label+" retains complete finite trajectories"));
        results[run.label]=r;
    }
    for spec in [["Reference",245.0],["Hot",199.675],["Cold",290.325],
        ["HotNameplate",220.05],["ColdNameplate",319.95],["HighBus",245.0],["ExtremeHot",0.0]] {
        let s=results[spec[0]].series;
        checks.push(t_true(near_all(s.available_w,spec[1],1.0e-7),spec[0]+" follows its temperature-adjusted rating"));
        checks.push(t_true(near_all(s.energy_balance_w,0.0,1.0e-8)
            && bounded(s.heat_w,0.0,2000.0,1.0e-8),spec[0]+" conserves radiant, electrical and thermal power"));
    }
    checks.push(t_true(near_all(results.HighBus.series.delivered_w,245.0,1.0e-7),
        "high bus voltage cannot multiply the sunlight power budget"));
    for label in ["StrongCooling","WeakCooling","Cooldown","BelowSink"] {
        checks.push(t_true(near_all(results[label].series.thermal_balance_j,0.0,0.001),
            label+" conserves integrated panel heat and rejected energy"));
    }
    for spec in [["StrongCooling",600.0],["WeakCooling",100.0],["Cooldown",100.0],
        ["BelowSink",100.0],["DisconnectedHeat",100.0]] {
        checks.push(t_true(bounded(results[spec[0]].series.cooling_w,0.0,spec[1],1.0e-8),
            spec[0]+" respects cooling capacity"));
    }
    let strong=results.StrongCooling.series;let weak=results.WeakCooling.series;
    checks.push(t_true(last(strong.temperature_k)<strong.temperature_k[0]-1.0,"sufficient cooling lowers cell temperature"));
    checks.push(t_true(last(strong.available_w)>strong.available_w[0]+1.0,"cooling improves electrical output through temperature feedback"));
    checks.push(t_true(last(weak.temperature_k)>weak.temperature_k[0]+1.0,"insufficient cooling allows the panel to heat"));
    checks.push(t_true(last(weak.available_w)<weak.available_w[0]-1.0,"heating reduces electrical output through temperature feedback"));
    let cold=results.Cooldown.series;
    checks.push(t_true(bounded(cold.temperature_k,298.15,318.15,1.0e-7)
        && last(cold.temperature_k)<299.0,"cooling approaches its sink without crossing below it"));
    let below=results.BelowSink.series;
    checks.push(t_true(near_all(below.cooling_w,0.0,1.0e-8)
        && near_all(below.temperature_k,288.15,1.0e-7),"cooler is off when the initial panel is below its sink"));
    checks.push(t_true(abs(last(results.DisconnectedHeat.series.thermal_balance_j))>1.0,
        "the energy witness detects an omitted solar-to-thermal connection"));
    this.done=true;report_verdict(checks,"SOLAR TEMPERATURE","SOLAR_TEMPERATURE");
}
