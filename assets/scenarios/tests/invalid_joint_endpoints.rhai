// An aliased endpoint may resolve to the same body; reject it before native admission.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/World/Host"),find_path("/World/Bob")],entity_writes:[],query_reads:["ListOpenDocuments","QueryUsdPrim","QueryPhysicsState","GetDiagnostics"],required_inputs:[]}
}
fn on_start(me,ctx) {
    this.doc=();this.opened=false;this.started=false;this.lint_requested=false;this.lint_tick=0;
    expect_runtime_fault("usd-physics-joint-invalid");
}
fn on_tick(me,ctx) {
    if this.started {return;}
    let documents=query("ListOpenDocuments",#{});
    for document in documents.open_documents {
        if document.origin!=() && document.origin.path.ends_with("invalid_joint_endpoints.usda") {this.doc=document.doc_id;}
    }
    if this.doc==() {
        if !this.opened {this.opened=true;cmd("OpenFile",#{path:"lunco://scenes/tests/invalid_joint_endpoints.usda"});}
        if sim_tick()>120 {throw("the exact source document did not open");}
        return;
    }
    if query("QueryPhysicsState",#{id:find_path("/World/Host")}).physics_state_ready!=true
        || query("QueryPhysicsState",#{id:find_path("/World/Bob")}).physics_state_ready!=true {throw("the valid fixture was not admitted");}
    if !this.lint_requested {cmd("RunLint",#{});this.lint_requested=true;this.lint_tick=sim_tick();return;}
    if sim_tick()==this.lint_tick {return;}
    let report=query("GetDiagnostics",#{scope:"loaded_stages"});
    if report.available!=true || report.complete!=true {if sim_tick()-this.lint_tick>120 {throw("joint lint did not complete");}return;}
    let found=false;
    for finding in report.diagnostics {
        if finding.code=="joint-same-body" && finding.subject=="/World/LintOnlySelf" {found=true;}
    }
    if !found {throw("the aliased self-joint was not diagnosed by its owning lint rule");}
    this.started=true;
    let joint=query("QueryUsdPrim",#{doc_id:this.doc,path:"/World/Joint"});
    let result=cmd("ApplyUsdOps",#{doc_id:this.doc,parent_gen:joint.generation,label:"Exercise resolved self-joint rejection",
        ops:[#{SetRelationship:#{edit_target:"@runtime@",path:"/World/Joint",name:"physics:body1",targets:["/World/Host/Alias"]}}]});
    if result.ok!=true {throw("the malformed relationship stimulus was rejected before projection");}
}
