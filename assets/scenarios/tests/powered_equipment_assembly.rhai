// Observe the public boundary of a generated network assembled from reusable USD parts.
// Dynamic UVLO and invalid operating points are covered by storage_power_contracts.rhai.
fn simulation_dependencies(me,ctx) {
    let equipment=find_path("/PoweredEquipmentTest/Equipment");
    #{modelica_entities:[equipment],entity_reads:[equipment],entity_writes:[],query_reads:[],required_inputs:[]}
}
fn on_start(me,ctx) {this.equipment=find_path("/PoweredEquipmentTest/Equipment");this.done=false;}
fn on_tick(me,ctx) {
    if this.done || clock_snapshot().physics_elapsed_s<1.0 {return;}
    let names=["voltage_v","camera_power_w","data_bps","available_rf_w","rf_w","tx_heat_w",
        "tx_input_w","rate_bps","delivered_bps","tx_active","snr_valid","source_power_w"];
    let s=#{};
    for name in names {
        let v=get(this.equipment,name);
        if v==() || !v.is_finite() {
            fail_fast("missing/nonfinite network output "+name,"POWERED EQUIPMENT","POWERED_EQUIPMENT");this.done=true;return;
        }
        s[name]=v;
    }
    let checks=[
        t_true(abs(s.voltage_v-28.0)<1.0e-8,"authored source sets the actual bus voltage"),
        t_true(abs(s.camera_power_w-4.5)<1.0e-8,"assembled camera consumes rated active input"),
        t_true(abs(s.data_bps-15.0e6)<1.0e-6,"powered capture generates offered science telemetry"),
        t_true(abs(s.available_rf_w-5.0)<1.0e-8,"transmitter supplies on-state acquisition capability"),
        t_true(s.snr_valid==1.0,"positive carrier admits a valid RF budget"),
        t_true(s.rate_bps>1000.0,"radio computes capacity from the supplied carrier"),
        t_true(s.delivered_bps>1000.0 && s.delivered_bps<=s.rate_bps+1.0e-6,"actual telemetry service respects capacity"),
        t_true(s.tx_active>0.0 && s.tx_active<=1.0,"radio service commands a physical transmit duty"),
        t_true(abs(s.rf_w-s.available_rf_w*s.tx_active)<1.0e-8,"radiated power follows actual modem duty"),
        t_true(abs(s.tx_input_w-s.rf_w-s.tx_heat_w)<1.0e-8,"transmitter pin input equals RF plus heat"),
        t_true(abs(s.source_power_w-s.camera_power_w-s.tx_input_w)<1.0e-8,"authored source supplies both actual loads"),
        t_true(get(this.equipment,"nonexistent_output")==(),"missing telemetry does not fabricate a value")];
    this.done=true;report_verdict(checks,"POWERED EQUIPMENT","POWERED_EQUIPMENT");
}
