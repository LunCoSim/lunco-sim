// Standard collision enablement applies before standalone shape admission.
const TEST_KIND = "headless";
fn simulation_dependencies(me,ctx) {
    let paths=["Disabled","Enabled","DisabledUnsupportedGeometry","DisabledTrigger","EnabledTrigger","Body","ProbeDisabled","ProbeEnabled"];
    let entities=[];
    for path in paths {entities.push(find_path("/World/"+path));}
    #{modelica_entities:[],entity_reads:entities,entity_writes:[],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn state(name) {query("QueryPhysicsState",#{id:find_path("/World/"+name)})}
fn on_start(me,ctx) {this.started=sim_tick();this.done=false;}
fn on_tick(me,ctx) {
    if this.done || sim_tick()-this.started<180 {return;}
    let disabled=state("Disabled");let enabled=state("Enabled");
    let unsupported=state("DisabledUnsupportedGeometry");
    let trigger=state("DisabledTrigger");let enabled_trigger=state("EnabledTrigger");
    let body=state("Body");
    let through=state("ProbeDisabled");let stopped=state("ProbeEnabled");
    this.done=true;
    report_verdict([
        t_true(disabled.collider_present==false && disabled.body_mode==(),"disabled standalone geometry has no native collider or body"),
        t_true(enabled.collider_present==true && enabled.body_mode=="static","omitted collisionEnabled retains the standard true default"),
        t_true(unsupported.collider_present==false && unsupported.body_mode==(),"disabled geometry is excluded before shape cooking"),
        t_true(trigger.collider_present==false && trigger.body_mode==(),"a disabled trigger has no native sensor or body"),
        t_true(enabled_trigger.collider_present==true && enabled_trigger.body_mode=="static","an enabled trigger retains its native sensor"),
        t_true(body.body_mode=="kinematic" && body.collider_source_paths==["/World/Body/Child"],"disabling a body's own shape preserves its enabled child shape"),
        t_true(world_pos(find_path("/World/ProbeDisabled"))[1] < -0.8 && abs(through.linear_velocity_mps[1]+1.0)<1.0e-5,"a moving probe passes through disabled static geometry"),
        t_true(world_pos(find_path("/World/ProbeEnabled"))[1]>0.65 && world_pos(find_path("/World/ProbeEnabled"))[1]<0.75 && abs(stopped.linear_velocity_mps[1])<0.01,"enabled static geometry stops the control probe")
    ],"COLLISION ENABLED","COLLISION_ENABLED");
}
