// Assertions consume production trajectories; all continuous math stays in Modelica.
fn simulation_dependencies(me, params) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn window_near(times,values,a,b,expected,tolerance) {
    let count=0;
    for i in 0..times.len() {
        if times[i]>=a && times[i]<=b {
            count+=1;if abs(values[i]-expected)>tolerance {return false;}
        }
    }
    count>0
}
fn bounded(values,lo,hi,tolerance) {
    for value in values {if value<lo-tolerance || value>hi+tolerance {return false;}}
    true
}
fn on_start(me,params) {this.done=false;}
fn on_tick(me,params) {
    if this.done {return;}
    let status=query("RunStatus",#{experiment_id:params.run});
    if status.state=="failed" || status.state=="cancelled" {
        fail_fast(status.to_string(),"STORAGE POWER","STORAGE_POWER");this.done=true;return;
    }
    if status.state!="done" {return;}
    let diagnostics=["positive capacity and an initial fill within capacity",
        "positive capacity and an initial fill within capacity","traffic requests must be nonnegative",
        "traffic requests must be nonnegative","requires nonnegative input demand",
        "0 < eta_tx <= 1","requires 0 < voltage_off_v < voltage_on_v",
        "requires p_active_w >= p_idle_w >= 0","requires nonnegative power ratings",
        "positive capacity and an initial fill within capacity",
        "time constants, range floor and transition widths must be positive"];
    let failed_runs=[];
    for run in params.invalid_runs {
        let failed=query("RunStatus",#{experiment_id:run});
        if failed.state!="failed" && failed.state!="done" && failed.state!="cancelled" {return;}
        failed_runs.push(failed);
    }
    let balances=["buffer_balance_mb","memory_balance_gb","link_balance_bits","source_balance_w",
        "radio_balance_w","bench_radio_balance_w","camera_balance_w","memory_power_balance_w",
        "computer_balance_w","lamp_balance_w","thermal_balance_j"];
    // Each observation window avoids event endpoints and checks every retained sample.
    let specs=[
        ["buffer.buffer_mb",1.1,1.9,1.0,1.0e-8],
        ["buffer.dropped_rate_kbps",1.1,1.9,8000.0,1.0e-6],
        ["buffer.buffer_mb",2.6,3.9,0.0,1.0e-8],
        ["buffer.unserved_rate_kbps",2.6,3.9,16000.0,1.0e-6],
        ["buffer.delivered_rate_kbps",4.1,5.4,4000.0,1.0e-6],
        ["buffer.buffer_mb",4.1,5.4,0.0,1.0e-8],
        ["buffer.buffer_mb",6.1,7.0,0.25,1.0e-8],
        ["full_buffer.buffer_mb",0.0,7.0,1.0,1.0e-8],
        ["full_buffer.delivered_rate_kbps",0.0,7.0,4000.0,1.0e-6],
        ["full_buffer.dropped_rate_kbps",0.0,7.0,0.0,1.0e-6],
        ["memory.stored_gb",1.6,3.4,1.0,1.0e-8],
        ["memory.rejected_write_rate_gbps",0.0,0.4,8.0,1.0e-8],
        ["memory.accepted_write_rate_gbps",1.6,1.9,0.0,1.0e-8],
        ["memory.stored_gb",4.1,5.9,0.5,1.0e-8],
        ["memory.delivered_read_rate_gbps",4.1,5.9,0.0,1.0e-8],
        ["memory.unserved_read_rate_gbps",4.1,5.9,8.0,1.0e-8],
        ["memory.stored_gb",6.6,7.0,0.0,1.0e-8],
        ["memory.power_draw_w",6.6,7.0,0.5,1.0e-8],
        ["radio.powered",1.6,1.9,1.0,1.0e-8],
        ["radio.powered",3.1,3.4,0.0,1.0e-8],
        ["radio.powered",6.1,7.0,1.0,1.0e-8],
        ["bench_radio.available_rf_power_w",0.6,1.9,5.0,1.0e-8],
        ["bench_radio.rf_power_out_w",0.6,1.9,2.5,1.0e-8],
        ["bench_radio.heat_w",0.6,1.9,5.25,1.0e-8],
        ["camera.data_rate_out_bps",0.6,1.9,15.0e6,1.0e-6],
        ["camera.data_rate_out_bps",6.1,7.0,0.0,1.0e-6],
        ["camera.power_draw_w",6.1,7.0,0.2,1.0e-8],
        ["computer.processing_active",0.6,1.9,1.0,1.0e-8],
        ["lamp.enabled",0.6,1.9,1.0,1.0e-8]];
    let off_names=["radio.power_draw_w","radio.rf_power_out_w","bench_radio.available_rf_power_w",
        "camera.data_rate_out_bps","computer.processing_active","lamp.light_intensity",
        "memory.power_draw_w","link.rate_bps","link.delivered_rate_bps","link.snr_valid"];
    let names=balances+off_names;
    for spec in specs {if !names.contains(spec[0]) {names.push(spec[0]);}}
    for name in ["link.buffer_bits","link.tx_active","link.lost_bits","body_temperature_k"] {
        if !names.contains(name) {names.push(name);}
    }
    let result=query("GetExperimentResult",#{experiment_id:params.run,variables:names,max_points:64});
    if result.series==() || result.times==() || result.times.is_empty() {
        fail_fast("missing storage/power trajectories","STORAGE POWER","STORAGE_POWER");this.done=true;return;
    }
    let s=result.series;let times=result.times;let complete=true;
    for name in names {
        complete=complete && s.contains(name) && s[name].len()==times.len();
        if s.contains(name) {for value in s[name] {complete=complete && value.is_finite();}}
    }
    if !complete {fail_fast("incomplete or nonfinite trajectories","STORAGE POWER","STORAGE_POWER");this.done=true;return;}
    let checks=[t_true(complete && times[0]==0.0 && abs(times[times.len()-1]-7.0)<1.0e-8,
        "complete finite trajectories cover initial outage through restart")];
    for name in balances {
        checks.push(t_true(window_near(times,s[name],0.0,7.0,0.0,1.0e-5),name+" conserves its domain"));
    }
    for spec in specs {
        checks.push(t_true(window_near(times,s[spec[0]],spec[1],spec[2],spec[3],spec[4]),
            spec[0]+" over ["+spec[1]+","+spec[2]+"]"));
    }
    for name in off_names {
        let off=true;
        for interval in [[0.0,0.4],[2.1,3.4],[4.1,5.9]] {
            off=off && window_near(times,s[name],interval[0],interval[1],0.0,1.0e-8);
        }
        checks.push(t_true(off,name+" is zero at absent, brownout and reverse bus voltage"));
    }
    checks.push(t_true(bounded(s["buffer.buffer_mb"],0.0,1.0,1.0e-8)
        && bounded(s["memory.stored_gb"],0.0,1.0,1.0e-8)
        && bounded(s["link.buffer_bits"],0.0,8.0e6,1.0e-5),"storage states remain within physical capacities"));
    checks.push(t_true(bounded(s["link.tx_active"],0.0,1.0,1.0e-8),"modem utilization stays within available carrier duty"));
    checks.push(t_true(s["link.lost_bits"][times.len()-1]>1.0e6,"overflow is counted as rejected traffic"));
    let final_buffer=s["link.buffer_bits"][times.len()-1];
    checks.push(t_true(final_buffer>0.0 && final_buffer<8.0e6,"restored RF drains retained telemetry after camera stops"));
    checks.push(t_true(s.body_temperature_k[times.len()-1]>250.0,"actual dissipated equipment power heats its thermal mass"));
    for i in 0..failed_runs.len() {
        let failed=failed_runs[i];
        checks.push(t_true(failed.state=="failed" && failed.error!=() && failed.error.contains(diagnostics[i])
            && (i!=3 || !failed.error.contains("at t=0.000000000")),"invalid storage/power operating point fails at its owner"));
    }
    this.done=true;report_verdict(checks,"STORAGE POWER","STORAGE_POWER");
}
