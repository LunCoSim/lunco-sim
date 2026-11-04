// Checked source closure, explicit class selection and initialized parameters.
// @timing presentation
const TEST_KIND = "editor";
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["CompileStatus","SnapshotVariables","GetDocumentSource"],required_inputs:[]}
}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn finish(state, checks) {
    state.done=true;
    report_verdict(checks,"MODELICA CHECKED LIFECYCLE","MODELICA_CHECKED_LIFECYCLE");
    for doc in state.docs {
        if state.saved && doc==state.doc {editor::close(doc);}
        else {editor::close_scratch(doc);}
    }
    state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",docs:[],checks:[],saved:false};
    try {
    if ctx.save_as_path==() || ctx.save_as_path=="" {throw("an explicit cache SaveAs path is required");}
    if ctx.save_as_invalid_path==() || ctx.save_as_invalid_path=="" {throw("an invalid SaveAs directory destination is required");}
    this.save_as_path=ctx.save_as_path;
    this.save_as_invalid_path=ctx.save_as_invalid_path;
    let helper=editor::new_scratch("model CheckedLifecycleHelper output Real value; equation value=4; end CheckedLifecycleHelper;","Checked helper");
    if helper.ok!=true {throw("helper scratch source was not created: "+helper);}
    this.helper=helper.doc_id;this.docs.push(helper.doc_id);
    let main=editor::new_scratch(`package CheckedLifecycle
      model Base parameter Real inherited=2*3; end Base;
      model First extends Base; output Real value; equation value=1; end First;
      model Second
        extends Base;
        parameter Real gain=2*3;
        parameter Integer count=2;
        final parameter Real fixedGain=4;
        parameter Boolean enabled=true;
        parameter String label="checked";
        protected parameter Real hidden=7;
        public parameter Real solved(start=1,fixed=false);
        CheckedLifecycleHelper helper;
        output Real value;
      initial equation solved=11;
      equation value=gain+inherited+helper.value+solved+count+hidden+fixedGain+(if enabled then 1 else 0);
      end Second;
    end CheckedLifecycle;`,"Checked selected class");
    if main.ok!=true {throw("main scratch source was not created: "+main);}
    this.doc=main.doc_id;this.docs.push(main.doc_id);
    } catch(error) {
        this.done=true;this=finish(this,["checked lifecycle startup failed: "+error]);
    }
}
fn on_tick(me, ctx) {
    if this.done {return;}
    try {
    this.elapsed+=execution_context().delta_seconds;
    if this.elapsed>120.0 {this=finish(this,["checked lifecycle timed out at "+this.phase]);return;}
    if this.phase=="parse" {
        for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return;}}
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"CheckedLifecycle.First",resume_after_compile:false});
        this.phase="first";return;
    }
    if this.phase=="first" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["first class did not compile: "+result]);return;}
        this.first=find("CheckedLifecycle.First");if this.first<0 {return;}
        this.first_source=query("GetDocumentSource",#{doc_id:this.doc});
        this.first_session=get(this.first,"ModelicaModel.session_id");
        this.pending=cmd("RunActiveModel",#{doc_id:this.doc,class:"CheckedLifecycle.Missing"});
        this.phase="invalid-class";return;
    }
    if this.phase=="invalid-class" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(result.ok==false && result.error!=() && result.error.contains("Missing")
            && sample.compiled && sample.paused && sample.model_name=="CheckedLifecycle.First"
            && sample.variables.value==1.0 && get(this.first,"ModelicaModel.session_id")==this.first_session
            && query("GetDocumentSource",#{doc_id:this.doc}).generation==this.first_source.generation,
            "invalid explicit Run class returns terminal rejection and preserves the installed source/session"));
        cmd("RunActiveModel",#{doc_id:this.doc,class:"CheckedLifecycle.Second"});
        this.phase="selected";return;
    }
    if this.phase=="selected" {
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        if sample.model_name!="CheckedLifecycle.Second" || sample.compiled!=true || sample.paused {return;}
        this.entity=find("CheckedLifecycle.Second");if this.entity<0 {return;}
        this.checks.push(t_true(this.entity==this.first && find("CheckedLifecycle.First")==-1,
            "explicit Run selects the requested class and refreshes the same document actor name"));
        this.checks.push(t_true(sample.variables.value==41.0,"the explicitly requested class owns the running observation"));
        this.checks.push(t_true(sample.parameters.gain==6.0 && sample.parameters.inherited==6.0,
            "checked inventory includes expression and inherited defaults"));
        this.checks.push(t_true(sample.parameters.solved==11.0 && sample.parameters.enabled==1.0
            && !sample.parameters.contains("label"),
            "numeric inventory uses initialization and Boolean facts while String retains its own type"));
        cmd("PauseActiveModel",#{doc_id:this.doc});
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"CheckedLifecycle.Second",force:true,resume_after_compile:false});
        this.phase="idle";return;
    }
    if this.phase=="idle" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {throw("paused compile checkpoint failed: "+result);}
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status.is_compiling || !status.paused || !status.is_compiled {
            throw("terminal paused compile did not publish its checked activity state: "+status);
        }
        this.before=query("GetDocumentSource",#{doc_id:this.doc});
        this.session=get(this.entity,"ModelicaModel.session_id");
        this.values=query("SnapshotVariables",#{doc_id:this.doc}).parameters;
        this.negative=0;
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"count",value:1.5});
        this.phase="negative";return;
    }
    if this.phase=="negative" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let after=query("GetDocumentSource",#{doc_id:this.doc});
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let named=false;
        if result.error!=() {
            named=if this.negative==0 {result.error.contains("Integer")} else {result.error.contains("public, non-final")};
        }
        this.checks.push(t_true(result.ok==false && named && after.generation==this.before.generation
            && after.source==this.before.source && get(this.entity,"ModelicaModel.session_id")==this.session
            && sample.parameters==this.values && sample.compiled,"invalid typed/protected/final edit rejects before source or session mutation"));
        this.negative+=1;
        if this.negative==1 {this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"hidden",value:8.0});return;}
        if this.negative==2 {this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"fixedGain",value:8.0});return;}
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:9.0});
        this.phase="expression";return;
    }
    if this.phase=="expression" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["expression parameter edit failed: "+result]);return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let authored=query("GetDocumentSource",#{doc_id:this.doc});
        this.checks.push(t_true(authored.source.contains("parameter Real gain=9.0;")
            && !authored.source.contains("gain=9.0*3"),
            "expression parameter authoring replaces the complete binding before any override can mask it"));
        this.checks.push(t_true(sample.parameters.gain==9.0 && sample.parameters.inherited==6.0
            && sample.parameters.solved==11.0 && sample.variables.value==44.0,
            "expression parameter replacement preserves inherited and initialization-solved values"));
        this.before=query("GetDocumentSource",#{doc_id:this.doc});
        this.session=get(this.entity,"ModelicaModel.session_id");
        let helper=query("GetDocumentSource",#{doc_id:this.helper});
        let edit=editor::replace_source(this.helper,"model CheckedLifecycleHelper output Real value; equation value=8; end CheckedLifecycleHelper;",helper.generation);
        if edit.ok!=true {this=finish(this,["helper edit failed: "+edit]);return;}
        this.phase="stale";return;
    }
    if this.phase=="stale" {
        if query("CompileStatus",#{doc_id:this.helper}).ast_parsed!=true {return;}
        this.checks.push(t_true(query("CompileStatus",#{doc_id:this.doc}).stale,
            "a changed sibling invalidates the primary participant's source closure"));
        let rejected=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:10.0});
        this.checks.push(t_true(rejected.ok==false && rejected.error.contains("source closure changed")
            && query("GetDocumentSource",#{doc_id:this.doc}).generation==this.before.generation,
            "parameter reconstruction rejects a stale sibling closure before mutation"));
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"CheckedLifecycle.Second",resume_after_compile:false});
        this.phase="recompiled";return;
    }
    if this.phase=="recompiled" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["sibling recompile failed: "+result]);return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(sample.variables.value==48.0 && sample.parameters.gain==9.0
            && get(this.entity,"ModelicaModel.session_id")>this.session,
            "unchanged primary source recompiles against the edited sibling; sample="+sample
            +"; session="+get(this.entity,"ModelicaModel.session_id")+"; previous_session="+this.session+"; result="+result));
        this.checks.push(t_true(query("CompileStatus",#{doc_id:this.doc}).stale==false,
            "successful recompilation publishes the exact current closure as fresh"));
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"CheckedLifecycle.Second",force:true,resume_after_compile:false});
        this.phase="warm";return;
    }
    if this.phase=="warm" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["warm source compile failed: "+result]);return;}
        let before=query("GetDocumentSource",#{doc_id:this.doc});
        let session=get(this.entity,"ModelicaModel.session_id");
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let rejected=cmd("SetModelicaParameter",#{entity:this.entity,key:"count",value:1.5});
        this.checks.push(t_true(rejected.ok==false && rejected.error!=() && rejected.error.contains("Integer")
            && sample.compiled && sample.variables.value==48.0 && sample.parameters.solved==11.0
            && get(this.entity,"ModelicaModel.session_id")==session
            && query("GetDocumentSource",#{doc_id:this.doc}).generation==before.generation,
            "warm source artifact reuse reattaches checked types before rejecting an invalid edit; sample="+sample
            +"; rejection="+rejected+"; session_before="+session+"; session_after="+get(this.entity,"ModelicaModel.session_id")
            +"; generation_before="+before.generation+"; source_after="+query("GetDocumentSource",#{doc_id:this.doc})));
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:10.0});
        this.phase="warm-edit";return;
    }
    if this.phase=="warm-edit" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(result.ok==true && sample.compiled && sample.parameters.gain==10.0
            && sample.parameters.inherited==6.0 && sample.parameters.solved==11.0 && sample.variables.value==49.0,
            "a cached compile retains editable source authority and initialized context for the next transaction"));
        this.before=query("GetDocumentSource",#{doc_id:this.doc});
        this.session=get(this.entity,"ModelicaModel.session_id");
        this.values=sample.parameters;
        this.pending=editor::save_as(this.doc,this.save_as_invalid_path);
        this.saveas_started=this.elapsed;
        this.saveas_reported=false;
        this.phase="saveas-failed";return;
    }
    if this.phase=="saveas-failed" {
        let result=terminal(this.pending);
        if this.elapsed-this.saveas_started>10.0 {throw("invalid SaveAs did not return a terminal failure: "+result);}
        if result.status=="pending" {return;}
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let rejected=result.ok==false && result.error!=() && result.error.contains(this.save_as_invalid_path);
        let source_unchanged=source.source==this.before.source && source.generation==this.before.generation
            && source.origin==this.before.origin;
        let session_unchanged=get(this.entity,"ModelicaModel.session_id")==this.session
            && query("SnapshotVariables",#{doc_id:this.doc}).parameters==this.values;
        this.save_failure_checked=rejected && source_unchanged && session_unchanged;
        this.pending=editor::save_as(this.doc,this.save_as_path);
        this.saveas_started=this.elapsed;
        this.phase="saveas";return;
    }
    if this.phase=="saveas" {
        let result=terminal(this.pending);
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let detail="pending="+this.pending+"; terminal="+result+"; origin="+source.origin+"; expected_path="+this.save_as_path;
        if !this.saveas_reported {print("CHECKED_LIFECYCLE_SAVEAS "+detail);this.saveas_reported=true;}
        if this.elapsed-this.saveas_started>10.0 {throw("SaveAs did not complete: "+detail);}
        if result.status=="pending" {return;}
        if result.ok!=true {throw("SaveAs admission failed: "+result);}
        if source.origin.kind!="file" || source.origin.path!=this.save_as_path {return;}
        this.saved=true;
        this.checks.push(t_true(this.save_failure_checked && source.source==this.before.source && source.generation==this.before.generation
            && query("CompileStatus",#{doc_id:this.doc}).stale,
            "SaveAs failure is terminal and preserves state; success changes only the compiler URI and invalidates reuse"));
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:11.0});
        this.phase="saveas-reject";return;
    }
    if this.phase=="saveas-reject" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let rejected=result.ok==false && result.error!=() && result.error.contains("source closure changed");
        let source_unchanged=source.generation==this.before.generation && source.source==this.before.source;
        let authority_unchanged=source.origin.path==this.save_as_path
            && get(this.entity,"ModelicaModel.session_id")==this.session;
        let observations_unchanged=sample.parameters==this.values && sample.compiled && sample.variables.value==49.0;
        this.checks.push(t_true(rejected && source_unchanged && authority_unchanged && observations_unchanged,
            "a relocated source rejects stale parameter authority without changing source or installed session"));
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"CheckedLifecycle.Second",resume_after_compile:false});
        this.phase="saveas-compiled";return;
    }
    if this.phase=="saveas-compiled" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {throw("relocated source compile failed: "+result);}
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let source_unchanged=source.source==this.before.source && source.generation==this.before.generation;
        let authority_current=source.origin.path==this.save_as_path
            && !query("CompileStatus",#{doc_id:this.doc}).stale;
        let session_replaced=sample.compiled && sample.model_name=="CheckedLifecycle.Second"
            && get(this.entity,"ModelicaModel.session_id")>this.session;
        let values_retained=sample.parameters.gain==10.0 && sample.parameters.inherited==6.0
            && sample.parameters.solved==11.0 && sample.variables.value==49.0;
        this.checks.push(t_true(source_unchanged && authority_current && session_replaced && values_retained,
            "recompiling the exact class admits its new URI and retains initialized values"));
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:11.0});
        this.phase="saveas-edit";return;
    }
    if this.phase=="saveas-edit" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let expected=this.before.source;
        expected.replace("parameter Real gain=10.0;","parameter Real gain=11.0;");
        let source_changed=source.generation>this.before.generation
            && source.source.contains("parameter Real gain=11.0;") && source.source==expected;
        let authority_current=source.origin.path==this.save_as_path
            && !query("CompileStatus",#{doc_id:this.doc}).stale;
        let values_updated=sample.parameters.gain==11.0 && sample.parameters.inherited==6.0
            && sample.parameters.solved==11.0 && sample.variables.value==50.0;
        this.last_edit_valid=result.ok==true && sample.compiled && source_changed && authority_current && values_updated;
        this.last_source=source;
        this.last_session=get(this.entity,"ModelicaModel.session_id");
        this.pending=editor::save(this.doc);
        this.saveas_started=this.elapsed;
        this.phase="saved-edit";return;
    }
    if this.phase=="saved-edit" {
        let result=terminal(this.pending);
        if this.elapsed-this.saveas_started>10.0 {throw("SaveDocument did not return a terminal result: "+result);}
        if result.status=="pending" {return;}
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        let source_retained=source.origin.path==this.save_as_path && source.source==this.last_source.source
            && source.generation==this.last_source.generation && source.dirty==false;
        let session_retained=get(this.entity,"ModelicaModel.session_id")==this.last_session
            && query("SnapshotVariables",#{doc_id:this.doc}).variables.value==50.0;
        this.checks.push(t_true(this.last_edit_valid && result.ok==true && source_retained && session_retained,
            "a valid edit changes only its selected binding and SaveDocument terminally saves that exact source"));
        this=finish(this,this.checks);
    }
    } catch(error) {
        this.done=true;this=finish(this,["checked lifecycle failed at "+this.phase+": "+error]);
    }
}
