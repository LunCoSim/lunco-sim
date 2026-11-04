// @timing presentation
const TEST_KIND="editor";
const NETWORK="/World/ForceBody/SixthBoundary";
const MOUNT=NETWORK+"/Mount";
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["CompileStatus","SnapshotVariables","GetDocumentSource","GeneratedModelicaSource",
        "GetReadiness","CosimStatus","GetPort","QueryPhysicsState"],required_inputs:[]}
}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn near(value,expected) {value!=() && value.is_finite() && abs(value-expected)<0.00000001}
fn attr(path,name,kind,value) {
    #{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}
}
fn add(parent,name,schemas) {[
    #{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:"Xform"}},
    #{SetApiSchemas:#{edit_target:"@runtime@",path:parent+"/"+name,schemas:schemas}}]}
fn coupled_fixture() {
    let ops=add("/World/ForceBody","SixthBoundary",["LunCoDomainSynthesisAPI","CollectionAPI:components"]);
    ops.push(attr(NETWORK,"lunco:synthesizer","token","\"actuator-wrench\""));
    ops.push(attr(NETWORK,"collection:components:expansionRule","token","\"explicitOnly\""));
    ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:NETWORK,name:"collection:components:includes",targets:[MOUNT]}});
    for name in ["desired_force_x","desired_force_y","desired_force_z","desired_torque_x","desired_torque_y","desired_torque_z"] {
        ops.push(attr(NETWORK,"inputs:"+name,"double","0"));
    }
    ops.push(attr(NETWORK,"outputs:valve","double","0"));
    for op in add(NETWORK,"Mount",["LunCoForceActuatorAPI"]) {ops.push(op);}
    ops.push(attr(MOUNT,"lunco:forceActuator:direction","float3","(0,1,0)"));
    ops.push(attr(MOUNT,"lunco:forceActuator:maxForce","double","10"));
    ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:MOUNT,name:"lunco:forceActuator:commandSource",targets:[NETWORK+".outputs:valve"]}});
    ops
}
fn generated() {
    let rows=query("GeneratedModelicaSource",#{network_root:NETWORK,include_source:false}).networks;
    if rows.len==1 {return rows[0];}()
}
fn coupled_row() {
    for row in query("CosimStatus").entities {if row.name==NETWORK {return row;}}()
}
fn finish(current,error) {
    if error!=() {current.checks.push(error.to_string());}
    current.done=true;
    report_verdict(current.checks,"MODELICA SIXTH PASS","MODELICA_SIXTH_PASS");
    for doc in current.docs {editor::close_scratch(doc);}
    current
}
fn on_start(me,ctx) {
    this=#{done:false,waiting:"source analysis",elapsed:0.0,docs:[],checks:[],pending:(),recent:false};
    try {
        if ctx.doc_id==() {throw("the existing simulation_core_contracts USD document id is required");}
        this.usd_doc=ctx.doc_id;
        let roots=editor::new_scratch("package Alpha end Alpha; package Beta end Beta;","Sixth namespaces");
        let primary=editor::new_scratch(`within Alpha; model Plant
          Beta.Plant helper; Beta.Extra extra;
          output Real baseValue; output Real extraValue; output Real y;
          equation baseValue=helper.gain; extraValue=extra.offset; y=baseValue+extraValue;
        end Plant;`,"Sixth qualified primary");
        let sibling=editor::new_scratch(`within Beta;
          model Plant parameter Real gain=3; end Plant;
          model Extra parameter Real offset=4; end Extra;`,"Sixth mixed sibling");
        for created in [roots,primary,sibling] {
            if created.ok!=true {throw("scratch creation failed: "+created);}
            this.docs.push(created.doc_id);
        }
        this.primary=primary.doc_id;this.sibling=sibling.doc_id;
    } catch(error) {this=finish(this,error);}
}
fn on_event(me,event,ctx) {
    if event.name=="COMPILE_DISPATCH_FAILED" && event.severity=="Error"
        && event.value.contains("duplicate Modelica declaration") && event.value.contains("Beta.Plant") {
        this.recent=true;
    }
}
fn sources_parsed(me) {
    if this.done {return false;}
    try {
        for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return false;}}
        return true;
    } catch(error) {this=finish(this,"source analysis failed: "+error);return false;}
}
fn compile_primary(me) {
    try {
        this.pending=cmd("CompileModel",#{doc_id:this.primary,class:"Alpha.Plant"});
        this.waiting="qualified compilation";
    } catch(error) {this=finish(this,"qualified compilation admission failed: "+error);}
}
fn command_finished(me) {
    if this.done {return false;}
    try {return terminal(this.pending).status!="pending";}
    catch(error) {this=finish(this,"command receipt failed at "+this.waiting+": "+error);return false;}
}
fn check_qualified_and_author_fixture(me) {
    try {
        let result=terminal(this.pending);
        let sample=query("SnapshotVariables",#{doc_id:this.primary});
        let selected=result.ok==true && sample.compiled && sample.model_name=="Alpha.Plant";
        this.checks.push(t_true(selected && near(sample.variables.baseValue,3.0),
            "distinct Alpha.Plant and Beta.Plant declarations coexist with their qualified identities"));
        this.checks.push(t_true(near(sample.variables.extraValue,4.0) && near(sample.variables.y,7.0),
            "the same sibling source retains all its unrelated mixed declarations"));
        if !selected {throw("qualified compilation failed: "+result);}
        this.pending=cmd("ApplyUsdOps",#{doc_id:this.usd_doc,ops:coupled_fixture()});
        cmd("SetTimeTransport",#{playing:true});
        this.waiting="coupled fixture admission and topology seal";
    } catch(error) {this=finish(this,"qualified fixture failed: "+error);}
}
fn fixture_ready(me) {
    if this.done {return false;}
    try {
        let receipt=terminal(this.pending);if receipt.status=="pending" {return false;}
        if receipt.ok!=true {throw("coupled fixture authoring failed: "+receipt);}
        let row=generated();if row==() || row.document_id==() {return false;}
        let ready=query("GetReadiness");if !ready.ready || ready.world_hold {return false;}
        if !query("CompileStatus",#{doc_id:row.document_id}).is_compiled {return false;}
        let synchronization=query("CosimStatus").synchronization;
        if !synchronization.topology_ready {return false;}
        this.generated=row;this.synchronization=synchronization;
        return true;
    } catch(error) {this=finish(this,"coupled fixture admission failed: "+error);return false;}
}
fn record_coupled(me) {
    try {
        let row=this.generated;
        this.checks.push(t_true(this.synchronization.shared_clock_participants>0 && row.document_source_matches,
            "the scene-owned generated solver participates in the shared Modelica clock"));
        this.boundary_doc=row.document_id;this.class=row.model_name;
        this.entity=find_path(NETWORK);
        this.waiting="outstanding shared-scene Step receipt";
    } catch(error) {this=finish(this,"coupled participant inspection failed: "+error);}
}
fn receipt_pending(me) {
    if this.done {return false;}
    try {
        let row=coupled_row();if row==() || !row.modelica_is_stepping {return false;}
        let clock=clock_snapshot();if !clock.barrier_held {return false;}
        this.step_row=row;this.step_clock=clock;
        return true;
    } catch(error) {this=finish(this,"pending Step inspection failed: "+error);return false;}
}
fn request_replacement(me) {
    try {
        this.session=get(this.entity,"ModelicaModel.session_id");
        this.checks.push(t_true(this.step_clock.barrier_held && this.step_row.modelica_is_stepping,
            "replacement is requested while a real shared-scene communication receipt is outstanding"));
        this.pending=cmd("CompileModel",#{doc_id:this.boundary_doc,class:this.class,force:true,resume_after_compile:false});
        this.checks.push(t_true(this.pending.status=="pending",
            "forced coupled compilation owns a pending terminal receipt"));
        this.waiting="coupled replacement installation";
    } catch(error) {this=finish(this,"replacement admission failed: "+error);}
}
fn check_replacement(me) {
    try {
        let result=terminal(this.pending);
        let sample=query("SnapshotVariables",#{doc_id:this.boundary_doc});
        let changed=get(this.entity,"ModelicaModel.session_id")!=this.session;
        this.checks.push(t_true(result.ok==true && changed && sample.compiled && sample.paused && sample.t==0.0,
            "checked replacement settles and installs a new coupled session"));
        let native=query("QueryPhysicsState",#{id:find_path("/World/ForceBody")});
        this.checks.push(t_true(native.computed_mass_finite && native.computed_inertia_finite
            && near(query("GetPort",#{api_id:this.entity,name:"valve"}).value,0.0),
            "coupled replacement preserves valid native state and its zero-demand physical output"));
        this.pending=cmd("ResumeActiveModel",#{doc_id:this.boundary_doc});
        this.waiting="resumed coupled solver advancement";
    } catch(error) {this=finish(this,"replacement inspection failed: "+error);}
}
fn resumed(me) {
    if this.done {return false;}
    try {
        let result=terminal(this.pending);if result.status=="pending" {return false;}
        let sample=query("SnapshotVariables",#{doc_id:this.boundary_doc});
        if result.ok==true && sample.t<=0.0 {return false;}
        this.resume_result=result;this.resume_sample=sample;
        return true;
    } catch(error) {this=finish(this,"resumed solver inspection failed: "+error);return false;}
}
fn check_resumed_and_add_duplicate(me) {
    try {
        let result=this.resume_result;let sample=this.resume_sample;
        this.checks.push(t_true(result.ok==true && sample.compiled && !sample.paused && sample.t>0.0,
            "the replaced coupled solver resumes through the normal communication owner"));
        cmd("PauseActiveModel",#{doc_id:this.boundary_doc});
        let duplicate=editor::new_scratch(`within Beta;
          model Plant parameter Real gain=99; end Plant;
          model Kept parameter Real offset=6; end Kept;`,"Sixth actual duplicate");
        if duplicate.ok!=true {throw("duplicate scratch admission failed: "+duplicate);}
        this.duplicate=duplicate.doc_id;this.docs.push(duplicate.doc_id);
        this.waiting="duplicate source analysis";
    } catch(error) {this=finish(this,"resumed solver fixture failed: "+error);}
}
fn duplicate_parsed(me) {
    if this.done {return false;}
    try {return query("CompileStatus",#{doc_id:this.duplicate}).ast_parsed==true;}
    catch(error) {this=finish(this,"duplicate source analysis failed: "+error);return false;}
}
fn compile_duplicate(me) {
    try {
        this.pending=cmd("CompileModel",#{doc_id:this.primary,class:"Alpha.Plant",force:true});
        this.waiting="duplicate rejection and Recent Events";
    } catch(error) {this=finish(this,"duplicate compilation admission failed: "+error);}
}
fn duplicate_finished(me) {
    if this.done {return false;}
    try {
        let result=terminal(this.pending);if result.status=="pending" {return false;}
        let rejected=result.ok==false && result.error!=() && result.error.contains("duplicate Modelica declaration 'Beta.Plant'");
        if rejected && !this.recent {return false;}
        this.duplicate_rejected=rejected;
        return true;
    } catch(error) {this=finish(this,"duplicate receipt inspection failed: "+error);return false;}
}
fn check_duplicate_and_report(me) {
    try {
        this.checks.push(t_true(this.duplicate_rejected,"actual duplicate qualified declarations reject explicitly instead of dropping a whole sibling"));
        let source=query("GetDocumentSource",#{doc_id:this.duplicate});
        this.checks.push(t_true(this.recent && source.source.contains("model Kept"),
            "Recent Events identifies the duplicate while the full mixed authored source remains intact"));
        this=finish(this,());
    } catch(error) {this=finish(this,"duplicate rejection inspection failed: "+error);}
}
fn task(me,ctx) {
    seq([
        wait_until(Fn("sources_parsed")), once(Fn("compile_primary")),
        wait_until(Fn("command_finished")), once(Fn("check_qualified_and_author_fixture")),
        wait_until(Fn("fixture_ready")), once(Fn("record_coupled")),
        wait_until(Fn("receipt_pending")), once(Fn("request_replacement")),
        wait_until(Fn("command_finished")), once(Fn("check_replacement")),
        wait_until(Fn("resumed")), once(Fn("check_resumed_and_add_duplicate")),
        wait_until(Fn("duplicate_parsed")), once(Fn("compile_duplicate")),
        wait_until(Fn("duplicate_finished")), once(Fn("check_duplicate_and_report"))
    ])
}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.elapsed+=execution_context().delta_seconds;
    if this.elapsed>110.0 {this=finish(this,"timeout at "+this.waiting+" pending="+terminal(this.pending));}
}
