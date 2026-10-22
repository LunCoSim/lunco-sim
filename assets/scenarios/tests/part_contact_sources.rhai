// A structural part observes its own cooked contacts, not its entire body.
const TEST_KIND = "headless";
const ROOT = "/World";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[find_path(ROOT+"/Observer")],entity_reads:[find_path(ROOT+"/Body"),find_path(ROOT+"/Body/Lower"),find_path(ROOT+"/Body/Upper"),find_path(ROOT+"/Body/Disabled")],entity_writes:[],
      query_reads:["ReadPorts"],required_inputs:[]}
}
fn observed(id) {
    let result=query("ReadPorts",#{api_id:id,port_names:["contact","contact_force","contact_approach_speed"]});
    if result.error!=() {throw(result.error);}
    let values=#{};
    for p in result.ports {
        if p.direction=="out" {
            if p.metadata.source!="Avian contact solver" {throw("contact observation has the wrong owner: "+p);}
            values[p.name]=p.value;
        }
    }
    if values.len()!=3 {throw("incomplete native contact surface");}
    values
}
fn on_start(me,ctx) {
    this.started=sim_tick();this.done=false;
    this.peak_lower=0.0;this.peak_body=0.0;this.peak_upper=0.0;
}
fn on_tick(me,ctx) {
    if this.done {return;}
    let body=find_path(ROOT+"/Body");let lower=find_path(ROOT+"/Body/Lower");let upper=find_path(ROOT+"/Body/Upper");
    track_entity_read(body);track_entity_read(lower);track_entity_read(upper);
    let b=();let lo=();let hi=();let error=();
    try {b=observed(body);lo=observed(lower);hi=observed(upper);} catch(e) {error=e;}
    if error!=() {
        this.done=true;report_verdict([t_true(false,"part contact observation failed: "+error)],"PART CONTACT SOURCES","PART_CONTACT_SOURCES");return;
    }
    this.peak_lower=max(this.peak_lower,lo.contact_approach_speed);
    this.peak_body=max(this.peak_body,b.contact_approach_speed);
    this.peak_upper=max(this.peak_upper,hi.contact_approach_speed);
    if sim_tick()-this.started<120 {return;}
    let disabled=find_path(ROOT+"/Body/Disabled");track_entity_read(disabled);
    let unavailable=false;
    try {
        let r=query("ReadPorts",#{api_id:disabled,port_names:["contact"]});
        unavailable=r.error!=() && r.error.contains("not declared");
    } catch(e) {unavailable=e.to_string().contains("not declared");}
    let checks=[
        t_true(lo.contact==1.0 && b.contact==1.0,"the supporting part and complete body report ground contact"),
        t_true(hi.contact==0.0 && hi.contact_force==0.0 && this.peak_upper==0.0,"an untouched sibling never inherits body impacts or load"),
        t_true(this.peak_lower>0.5 && this.peak_body==this.peak_lower,"closing speed belongs to the impacting part"),
        t_true(abs(lo.contact_force-196.2)<10.0 && abs(b.contact_force-lo.contact_force)<1.0e-6,"part load equals supported weight without double-counting body membership"),
        t_true(unavailable,"a disabled shape has no native contact observation"),
        t_true(abs(port(find_path(ROOT+"/Observer"),"supported_force")-(lo.contact_force-hi.contact_force))<1.0,"part contacts admit and drive an authored Modelica consumer")
    ];
    emit("PART_CONTACT_SOURCE_EVIDENCE",#{body:b,lower:lo,upper:hi,peak_lower:this.peak_lower});
    this.done=true;report_verdict(checks,"PART CONTACT SOURCES","PART_CONTACT_SOURCES");
}
