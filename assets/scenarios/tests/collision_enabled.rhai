// Standard collision enablement applies before standalone shape admission.
const TEST_KIND = "headless";
fn simulation_dependencies(me,ctx) {
    let paths=["Disabled","Enabled","DisabledUnsupportedGeometry","DisabledTrigger","EnabledTrigger","Body","ProbeDisabled","ProbeEnabled"];
    let entities=[];
    for path in paths {entities.push(find_path("/World/"+path));}
    #{modelica_entities:[],entity_reads:entities,entity_writes:[find_path("/World/Enabled")],query_reads:["QueryPhysicsState","QueryUsdPrim"],required_inputs:[]}
}
fn state(name) {query("QueryPhysicsState",#{id:find_path("/World/"+name)})}
fn on_start(me,ctx) {this.started=sim_tick();this.done=false;this.pending=();this.checks=[];}
fn on_tick(me,ctx) {
    if this.done || sim_tick()-this.started<180 {return;}
    if this.pending!=() {
        let result=command_result(this.pending);
        if result.status=="pending" {return;}
        if result.ok!=true {
            this.checks.push(t_true(false,"standalone shape edit rejected: "+result.to_string()));
        } else {
            if sim_tick()-this.edit_tick<30 {return;}
            let edited=state("Enabled");
            let target=find_path("/World/Enabled");
            track_entity_read(target);
            let bounds=edited.collider_local_bounds;
            this.checks.push(t_true(bounds!=() && abs(bounds.max[0]-0.4)<1e-6 && abs(bounds.min[0]+0.4)<1e-6,
                "localized dimension edit reaches the native collider"));
            this.checks.push(t_true(target==this.target && find_path("/World/ProbeEnabled")==this.unrelated &&
                edited.collider_present && edited.body_mode=="static",
                "localized dimension edit preserves the changed and unrelated entities"));
            this.checks.push(t_true(get(target,"Mesh3d").to_string()!=this.mesh,
                "localized dimension edit replaces only the primitive mesh"));
        }
        this.done=true;
        report_verdict(this.checks,"COLLISION ENABLED","COLLISION_ENABLED");
        return;
    }
    let disabled=state("Disabled");let enabled=state("Enabled");
    let unsupported=state("DisabledUnsupportedGeometry");
    let trigger=state("DisabledTrigger");let enabled_trigger=state("EnabledTrigger");
    let body=state("Body");
    let through=state("ProbeDisabled");let stopped=state("ProbeEnabled");
    let target=find_path("/World/Enabled");
    let missing=find_path("/World/Disabled");
    let authored=query("QueryUsdPrim",#{path:"/World/Enabled",attrs:["physics:collisionEnabled"]});
    let rejected=cmd("SetColliderEnabled",#{target:missing,enabled:true});
    let disable=cmd("SetColliderEnabled",#{target:target,enabled:false});
    let live_disabled=state("Enabled");
    let enable=cmd("SetColliderEnabled",#{target:target,enabled:true});
    let live_enabled=state("Enabled");
    let after=query("QueryUsdPrim",#{path:"/World/Enabled",attrs:["physics:collisionEnabled"]});
    this.checks=[
        t_true(!rejected.ok,"a source prim with no admitted collider rejects live enablement"),
        t_true(disable.ok && live_disabled.collider_disabled && live_disabled.collider_present,
            "runtime disable keeps the already admitted native collider"),
        t_true(enable.ok && !live_enabled.collider_disabled && live_enabled.collider_present &&
            find_path("/World/Enabled")==target,
            "runtime enable restores the same collider entity"),
        t_true(after.document_generation==authored.document_generation && after.attrs==authored.attrs,
            "live contact enablement leaves the USD document unchanged"),
        t_true(disabled.collider_present==false && disabled.body_mode==(),"disabled standalone geometry has no native collider or body"),
        t_true(enabled.collider_present==true && enabled.body_mode=="static","omitted collisionEnabled retains the standard true default"),
        t_true(unsupported.collider_present==false && unsupported.body_mode==(),"disabled geometry is excluded before shape cooking"),
        t_true(trigger.collider_present==false && trigger.body_mode==(),"a disabled trigger has no native sensor or body"),
        t_true(enabled_trigger.collider_present==true && enabled_trigger.body_mode=="static","an enabled trigger retains its native sensor"),
        t_true(body.body_mode=="kinematic" && body.collider_source_paths==["/World/Body/Child"],"disabling a body's own shape preserves its enabled child shape"),
        t_true(world_pos(find_path("/World/ProbeDisabled"))[1] < -0.8 && abs(through.linear_velocity_mps[1]+1.0)<1.0e-5,"a moving probe passes through disabled static geometry"),
        t_true(world_pos(find_path("/World/ProbeEnabled"))[1]>0.65 && world_pos(find_path("/World/ProbeEnabled"))[1]<0.75 && abs(stopped.linear_velocity_mps[1])<0.01,"enabled static geometry stops the control probe")
    ];
    this.target=target;this.unrelated=find_path("/World/ProbeEnabled");
    this.mesh=get(target,"Mesh3d").to_string();
    let result=cmd("ApplyUsdOp",#{doc_id:authored.doc_id,parent_gen:authored.document_generation,
        op:#{SetAttribute:#{edit_target:"@runtime@",path:"/World/Enabled",name:"size",type_name:"double",value:"0.8"}}});
    if result.ok!=true || result.id==() {
        this.checks.push(t_true(false,"standalone shape edit not admitted"));
        this.done=true;
        report_verdict(this.checks,"COLLISION ENABLED","COLLISION_ENABLED");
        return;
    }
    this.pending=result.id;this.edit_tick=sim_tick();
}
