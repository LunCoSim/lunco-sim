// Production energy, charge and finite-boundary acceptance for the shared battery.
// These synthetic benches qualify the stated idealization, not a flight OCV curve.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn last(values) { values[values.len()-1] }
fn near_all(values, target, tolerance) {
    for value in values { if abs(value-target)>tolerance { return false; } }
    true
}
fn nonnegative(values, tolerance) {
    for value in values { if value < -tolerance { return false; } }
    true
}
fn in_range(values, lo, hi, tolerance) {
    for value in values { if value<lo-tolerance || value>hi+tolerance { return false; } }
    true
}
fn window_near(result, name, start, end, target, tolerance) {
    let found=false;
    for i in 0..result.times.len() {
        if result.times[i]>=start && result.times[i]<=end {
            found=true;
            if abs(result.series[name][i]-target)>tolerance { return false; }
        }
    }
    found
}
fn energy_balance(result) {
    let s=result.series;
    let tolerance=1.0e-6+1.0e-7*abs(s.stored_energy_j[0]); // J; integration resolution
    for i in 0..result.times.len() {
        if abs(s.stored_energy_j[i]-s.stored_energy_j[0]
            -s.absorbed_energy_j[i]+s.heat_energy_j[i])>tolerance { return false; }
    }
    true
}
fn on_start(me, params) { this.done=false; }
fn on_tick(me, params) {
    if this.done { return; }
    for run in params.runs {
        let status=query("RunStatus",#{experiment_id:run.experiment_id});
        if status.state!="done" && status.state!="failed" && status.state!="cancelled" { return; }
    }
    let checks=[];
    let results=#{};
    for run in params.runs {
        let status=query("RunStatus",#{experiment_id:run.experiment_id});
        if run.invalid {
            checks.push(t_true(status.state=="failed" && status.error!=()
                && status.error.contains("Battery requires positive capacity and voltage"),
                run.label+" fails at the battery parameter assertion"));
            continue;
        }
        if status.state!="done" {
            this.done=true;fail_fast(status.to_string(),"BATTERY ENERGY","BATTERY_ENERGY");return;
        }
        let result=query("GetExperimentResult",#{experiment_id:run.experiment_id,
            variables:run.variables,max_points:4000});
        let complete=result.times!=() && result.times.len()>1 && result.series!=()
            && result.downsampled==false && result.missing_variables.is_empty()
            && result.times[0]==0.0 && abs(last(result.times)-run.stop_s)<1.0e-8;
        if complete {
            for name in run.variables {
                complete=complete && result.series.contains(name)
                    && result.series[name].len()==result.times.len();
                if result.series.contains(name) {
                    for value in result.series[name] { complete=complete && value.is_finite(); }
                }
            }
        }
        if !complete {
            this.done=true;fail_fast("incomplete production trajectory: "+run.label,
                "BATTERY ENERGY","BATTERY_ENERGY");return;
        }
        checks.push(t_true(complete,run.label+" has complete finite native trajectories"));
        results[run.label]=result;
    }
    let metrics=[];
    for label in ["ideal","lossy"] {
        let r=results[label];let s=r.series;
        let tolerance=1.0e-6+1.0e-7*s.nameplate_energy_j[0]; // J
        checks.push(t_true(near_all(s.charge_balance_ah,0.0,1.0e-8),label+" conserves charge"));
        checks.push(t_true(abs(s.stored_energy_j[0]-s.nameplate_energy_j[0])<tolerance,
            label+" starts with the complete nominal Wh budget"));
        checks.push(t_true(in_range(s.state_of_charge,0.0,1.0,1.0e-9),label+" bounds charge state"));
        checks.push(t_true(energy_balance(r),label+" conserves terminal, stored and thermal energy"));
        checks.push(t_true(last(s.stored_energy_j)<tolerance
            && abs(-last(s.absorbed_energy_j)+last(s.heat_energy_j)-s.nameplate_energy_j[0])<tolerance,
            label+" transfers the full storage budget through the empty reserve"));
        let loss_ok=true;
        for i in 0..r.times.len() {
            loss_ok=loss_ok && abs(s.heat_energy_j[i]-s.series_loss_j[i])<tolerance;
        }
        checks.push(t_true(loss_ok,label+" separately accounts for series loss"));
        metrics.push(#{probe:label,initial_stored_j:s.stored_energy_j[0],
            final_stored_j:last(s.stored_energy_j),bus_discharge_j:last(s.delivered_energy_j),
            net_bus_absorbed_j:last(s.absorbed_energy_j),heat_j:last(s.heat_energy_j),
            transferred_charge_ah:last(s.transferred_charge_ah)});
    }
    let cycle=results.cycle;
    checks.push(t_true(energy_balance(cycle),"full discharge/recharge/shunt cycle conserves energy"));
    checks.push(t_true(in_range(cycle.series.state_of_charge,0.0,1.0,1.0e-9),"cycle bounds charge state"));
    checks.push(t_true(window_near(cycle,"state_of_charge",1.1,1.4,0.0,1.0e-9)
        && window_near(cycle,"state_of_charge",2.6,4.0,1.0,1.0e-9),"empty holds and recharge reaches full"));
    checks.push(t_true(window_near(cycle,"terminal_power_w",1.1,1.4,0.1,1.0e-8),
        "forced reverse bus at empty absorbs resistor power instead of creating energy"));
    let shunt=true;let shunt_sampled=false;
    for i in 0..cycle.times.len() {
        if cycle.times[i]>=2.6 && cycle.times[i]<=3.4 {
            shunt_sampled=true;shunt=shunt && cycle.series.heat_w[i]>28.0;
        }
    }
    checks.push(t_true(shunt_sampled && shunt,"full-charge surplus is dissipated in the explicit ideal shunt"));
    checks.push(t_true(window_near(cycle,"terminal_power_w",3.6,4.0,0.0,1.0e-8)
        && window_near(cycle,"heat_w",3.6,4.0,0.0,1.0e-8),"zero current has no continued power or heat"));
    let regen=results.regeneration;
    checks.push(t_true(energy_balance(regen),"connected motor regeneration conserves battery energy"));
    checks.push(t_true(last(regen.series.stored_energy_j)>regen.series.stored_energy_j[0],
        "overspeed motor increases battery stored energy"));
    checks.push(t_true(nonnegative(regen.series.terminal_power_w,1.0e-8),
        "regeneration supplies positive power into the battery"));
    checks.push(t_true(nonnegative(regen.series.heat_w,1.0e-8),"regeneration has nonnegative heat"));
    for name in ["voltage_v","current_a","discharge_w","stored_j"] {
        checks.push(t_true(near_all(results.empty.series[name],0.0,1.0e-10),
            "empty passive circuit has zero "+name));
    }
    emit("BATTERY_ENERGY_METRICS",#{probes:metrics,model_fidelity:"idealized plateau/reserve and full-charge shunt; not flight calibration"});
    this.done=true;report_verdict(checks,"BATTERY ENERGY","BATTERY_ENERGY");
}
