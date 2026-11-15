const TEST_KIND="headless";
const ROOT="/ScenarioAccessInspection";

fn simulation_dependencies(me,ctx) {
    let bare=find_path(ROOT+"/Target");
    if usd_path(me)==ROOT+"/Worker" {
        return #{modelica_entities:[],entity_reads:[bare],entity_writes:[bare],
            query_reads:[],required_inputs:[]};
    }
    #{modelica_entities:[],entity_reads:[bare,find_path(ROOT+"/Worker")],
        entity_writes:[find_path(ROOT+"/Worker")],query_reads:["ScriptInspect"],required_inputs:[]}
}

fn on_start(me,ctx) {this.done=false;this.stage="inspect";this.started=sim_tick();this.checks=[];}

fn on_tick(me,ctx) {
    if this.done || usd_path(me)==ROOT+"/Worker" || sim_tick()-this.started<3 {return;}
    let worker=find_path(ROOT+"/Worker");
    let bare=find_path(ROOT+"/Target");
    let inspection=query("ScriptInspect",#{target:worker});
    if this.stage=="inspect" {
        let plan=inspection.access_plan;
        this.checks.push(t_true(inspection.scripted && plan!=(),"worker has an admitted access plan"));
        if plan==() {this.done=true;report_verdict(this.checks,"SCENARIO ACCESS INSPECTION","SCENARIO_ACCESS_INSPECTION");return;}
        this.checks.push(t_true(plan.semantics=="declared_access","inspection distinguishes authorization from transfers"));
        this.checks.push(t_true(plan.entity_reads.contains(bare) && plan.entity_writes.contains(bare),"both directional entity grants are exposed"));
        let target=query("ScriptInspect",#{target:bare});
        this.checks.push(t_true(!target.scripted && target.access_plan==(),"bare target does not fabricate an access plan"));
        let own=query("ScriptInspect",#{target:me}).access_plan;
        track_entity_read(bare);
        let unchanged=query("ScriptInspect",#{target:me}).access_plan;
        this.checks.push(t_true(own.revision==unchanged.revision,"repeated read admission does not change the plan revision"));
        track_entity_write(bare);
        let extended=query("ScriptInspect",#{target:me}).access_plan;
        this.checks.push(t_true(extended.entity_writes.contains(bare) && extended.revision>unchanged.revision,
            "runtime-discovered write admission extends the existing owner plan"));
        let stopped=cmd("StopScenario",#{target:worker});
        this.checks.push(t_true(stopped!=() && stopped.ok==true,"scenario retirement command is admitted"));
        this.revision=extended.revision;this.stage="retire";this.retire_tick=sim_tick();return;
    }
    if inspection.scripted || inspection.access_plan!=() {
        if sim_tick()-this.retire_tick<20 {return;}
        this.checks.push(t_true(false,"retired scenario releases its access plan"));
    } else {
        this.checks.push(t_true(true,"retired scenario releases its access plan"));
        this.checks.push(t_true(query("ScriptInspect",#{target:me}).access_plan.revision>this.revision,"retirement advances the owner revision"));
    }
    this.done=true;
    report_verdict(this.checks,"SCENARIO ACCESS INSPECTION","SCENARIO_ACCESS_INSPECTION");
}
