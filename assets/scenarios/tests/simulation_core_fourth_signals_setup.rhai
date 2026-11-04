// @timing presentation
// The driver supplies simulation_core_contracts' document, then installs an
// external controller hold before launching the Simulation-clock observer.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","GetPort"],required_inputs:[]}
}
fn attr(name,type_name,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:"/World/LayeredInput",
        name:name,type_name:type_name,value:value}}
}
fn on_start(me,ctx) {
    this.done=false;this.frames=0;this.receipt=();
    if ctx.doc_id==() {throw("signals setup requires the fixture doc_id");}
    this.receipt=cmd("ApplyUsdOps",#{doc_id:ctx.doc_id,ops:[
        #{AddPrim:#{edit_target:"@runtime@",parent_path:"/World",name:"LayeredInput",type_name:"Scope"}},
        #{SetApiSchemas:#{edit_target:"@runtime@",path:"/World/LayeredInput",schemas:["LunCoProgramAPI"]}},
        attr("info:implementationSource","token","\"sourceAsset\""),
        attr("info:sourceAsset","asset","@lunco://models/tests/LayeredModelInput.mo@"),
        attr("inputs:u","double","1"),attr("outputs:y","double","0") ]});
    if this.receipt.id==() {throw("signals setup received no edit receipt");}
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.frames+=1;
    if this.frames>1800 {
        this.done=true;report_verdict(["live Modelica input fixture did not become ready"],
            "SIMULATION CORE FOURTH SIGNALS SETUP","SIMULATION_CORE_FOURTH_SIGNALS_SETUP");return;
    }
    try {
        let result=command_result(this.receipt.id);
        if result.status=="pending" {return;}
        if result.ok!=true {throw("live Modelica fixture edit rejected: "+result);}
        let ready=query("GetReadiness");if ready.faulted {throw("live Modelica fixture faulted: "+ready);}
        let target=find_path("/World/LayeredInput");if target<0 {return;}
        let output=query("GetPort",#{api_id:target,name:"y"});
        if output.value==() || !ready.ready || ready.world_hold {return;}
        this.done=true;emit("SIMULATION_CORE_FOURTH_SIGNALS_READY",#{target_gid:target});
        report_verdict([t_true(abs(output.value-1.0)<1.0e-12,"live algebraic model initializes from u=1")],
            "SIMULATION CORE FOURTH SIGNALS SETUP","SIMULATION_CORE_FOURTH_SIGNALS_SETUP");
    } catch(error) {
        this.done=true;report_verdict([error.to_string()],
            "SIMULATION CORE FOURTH SIGNALS SETUP","SIMULATION_CORE_FOURTH_SIGNALS_SETUP");
    }
}
