// @timing presentation
const ROOT="/World/ForceBody/Allocator";
const CARRIER=ROOT+"/Carrier";
const MOUNT=CARRIER+"/Mount";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["GetReadiness","ListOpenDocuments","GetDocumentSource","CompileStatus"],required_inputs:[]}
}
fn a(path,name,kind,value) {#{SetAttribute:#{edit_target:"@runtime@",path:path,name:name,type_name:kind,value:value}}}
fn add(parent,name,schemas) {[
    #{AddPrim:#{edit_target:"@runtime@",parent_path:parent,name:name,type_name:"Xform"}},
    #{SetApiSchemas:#{edit_target:"@runtime@",path:parent+"/"+name,schemas:schemas}}]}
fn lever() {vec3_array(get(find_path(MOUNT),"ForceActuator.local_position"))[0]}
fn model_document() {
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind!="modelica" {continue;}
        let source=query("GetDocumentSource",#{doc_id:doc.doc_id});
        if source.source.contains("wrench_matrix =") && query("CompileStatus",#{doc_id:doc.doc_id}).is_compiled {return doc.doc_id;}
    }
    ()
}
fn finish(state) {
    state.done=true;report_verdict(state.checks,"SIMULATION CORE LIVE ALLOCATOR","SIMULATION_CORE_LIVE_ALLOCATOR");state
}
fn on_start(me,ctx) {this=#{done:false,phase:0,elapsed:0.0,doc:ctx.doc_id,checks:[]};}
fn on_tick(me,ctx) {
    if this.done {return;}
    try {
        this.elapsed+=execution_context().delta_seconds;
        if this.elapsed>90.0 {throw("live allocator timed out at phase "+this.phase);}
        if this.phase==0 {
            let ops=add("/World/ForceBody","Allocator",["LunCoDomainSynthesisAPI","CollectionAPI:components"]);
            ops.push(a(ROOT,"lunco:synthesizer","token","\"actuator-wrench\""));
            ops.push(a(ROOT,"collection:components:expansionRule","token","\"explicitOnly\""));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:ROOT,name:"collection:components:includes",targets:[MOUNT]}});
            ops.push(a(ROOT,"outputs:valve","double","0"));
            for op in add(ROOT,"Carrier",[]) {ops.push(op);}
            ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:CARRIER,value:[1.0,0.0,0.0]}});
            for op in add(CARRIER,"Mount",["LunCoForceActuatorAPI"]) {ops.push(op);}
            ops.push(#{SetTranslate:#{edit_target:"@runtime@",path:MOUNT,value:[1.0,0.0,0.0]}});
            ops.push(a(MOUNT,"lunco:forceActuator:direction","float3","(0, 1, 0)"));
            ops.push(a(MOUNT,"lunco:forceActuator:maxForce","double","10"));
            ops.push(#{SetRelationship:#{edit_target:"@runtime@",path:MOUNT,name:"lunco:forceActuator:commandSource",targets:[ROOT+".outputs:valve"]}});
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:ops}).id;this.phase=1;return;
        }
        let result=command_result(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {throw("allocator edit failed: "+result);}
        let ready=query("GetReadiness");if ready.faulted {throw("allocator fixture faulted: "+ready);}
        let entity=find_path(ROOT);if entity<0 || !ready.ready {return;}
        let doc=model_document();if doc==() {return;}
        if !query("CompileStatus",#{doc_id:doc}).is_compiled {return;}
        let source=query("GetDocumentSource",#{doc_id:doc});
        if this.phase==1 {
            this.checks.push(t_true(abs(lever()-2.0)<0.00001,"allocator physical member starts at its composed lever"));
            this.checks.push(t_true(source.source.contains("wrench_matrix ="),"allocator source contains the geometry matrix"));
            this.before=source;this.before_doc=doc;this.session=get(entity,"ModelicaModel.session_id");
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[#{SetTranslate:#{edit_target:"@runtime@",path:CARRIER,value:[2.0,0.0,0.0]}}]}).id;
            this.phase=2;return;
        }
        if this.phase==2 {
            if source.source==this.before.source || get(entity,"ModelicaModel.session_id")==this.session {return;}
            this.checks.push(t_true(abs(lever()-3.0)<0.00001,"ancestor edit refreshes native actuator lever"));
            this.checks.push(t_true(source.source!=this.before.source && (doc!=this.before_doc || source.generation>this.before.generation),
                "the same ancestor edit regenerates the allocator geometry source"));
            this.checks.push(t_true(get(entity,"ModelicaModel.session_id")!=this.session,"new generated geometry is installed in a replacement solver session"));
            this.after=source;this.session=get(entity,"ModelicaModel.session_id");
            this.pending=cmd("ApplyUsdOps",#{doc_id:this.doc,ops:[#{SetTranslate:#{edit_target:"@runtime@",path:"/World/ForceBody",value:[20.0,10.0,0.0]}}]}).id;
            this.phase=3;this.waited=0;return;
        }
        this.waited+=1;if this.waited<30 {return;}
        this.checks.push(t_true(source.source==this.after.source && get(entity,"ModelicaModel.session_id")==this.session
            && abs(lever()-3.0)<0.00001,"whole-assembly translation preserves allocator source, session and relative lever"));
        this=finish(this);
    } catch(error) {this.checks.push("live allocator failed: "+error);this=finish(this);}
}
