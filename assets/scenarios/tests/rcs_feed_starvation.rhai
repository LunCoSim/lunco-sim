// Production RCSJet behavior: valve demand remains open while either feed
// disappears. The model owns thrust, flow, activity and shared photometry.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[find_path("/World/Jet")],entity_reads:[],entity_writes:[],query_reads:[],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.jet=find_path("/World/Jet");this.phase=0;this.elapsed=0.0;this.done=false;this.failures=[];
}
fn feed(jet,writes) {
    let result=cmd("SetPorts",#{target:jet,writes:writes});
    if !result.ok {throw("RCS feed stimulus rejected");}
}
fn on_tick(me,ctx) {
    if this.done {return;}this.elapsed+=dt();if this.elapsed<0.25 {return;}this.elapsed=0.0;
    let force=get(this.jet,"thrust_n");let flow=get(this.jet,"mass_flow_kgs");
    let light=get(this.jet,"light_intensity");let activity=get(this.jet,"activity");
    if this.phase==0 || this.phase==2 {
        this.failures.push(t_true(force>100.0 && flow>0.0 && light>0.0 && activity>0.99,"available feeds did not produce thrust and light"));
        this.failures.push(t_true(get(this.jet,"envelope_radius_m")>0.0 && get(this.jet,"envelope_length_m")>0.0,"shared photometry envelope missing"));
        if this.phase==0 {feed(this.jet,[["available_fuel_mass_kg",0.0]]);}
        else {feed(this.jet,[["available_oxidizer_mass_kg",0.0]]);}
    } else {
        this.failures.push(t_true(get(this.jet,"valve_opening")>0.99,"valve demand was shut off by the test"));
        this.failures.push(t_true(force==0.0 && flow==0.0 && activity==0.0,"starved RCS engine continued firing"));
        this.failures.push(t_true(light==0.0 && get(this.jet,"visual_length_fraction")==0.0,"starved RCS engine left a plume"));
        if this.phase==1 {feed(this.jet,[["available_fuel_mass_kg",1.0]]);}
        else {this.done=true;report_verdict(this.failures,"RCS FEED STARVATION","RCS_FEED_STARVATION");return;}
    }
    this.phase+=1;
}
