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
    report_verdict(checks,"MODELICA THIRD PASS","MODELICA_THIRD_PASS");
    for doc in state.docs {
        if doc==state.good && state.saved {editor::close(doc);}
        else if doc!=state.retired {editor::close_scratch(doc);}
    }
    state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",checks:[],docs:[],saved:false,good:(),retired:()};
    try {
        if ctx.save_path==() || ctx.invalid_save_path==() {throw("explicit cache save paths are required");}
        this.save_path=ctx.save_path;this.invalid_save_path=ctx.invalid_save_path;
        let good=editor::new_scratch(`model ThirdPassGood
          parameter Real gain=2*3;
          Real x(start=1,fixed=true); output Real value;
          equation der(x)=4; value=x+gain;
        end ThirdPassGood;`,"Third pass good");
        let bad=editor::new_scratch("model ThirdPassBad Real x; equation x=MissingValue; end ThirdPassBad;","Third pass bad");
        let retired=editor::new_scratch("model ThirdPassRetired end ThirdPassRetired;","Third pass retired");
        if good.ok!=true || bad.ok!=true || retired.ok!=true {throw("scratch creation failed");}
        this.good=good.doc_id;this.bad=bad.doc_id;this.to_retire=retired.doc_id;
        this.docs=[this.good,this.bad,this.to_retire];
    } catch(error) {this=finish(this,["third-pass startup failed: "+error]);}
}
fn on_tick(me, ctx) {
    if this.done {return;}
    try {
        this.elapsed+=execution_context().delta_seconds;
        if this.elapsed>90.0 {throw("timed out at "+this.phase+"; pending="+this.pending);}
        if this.phase=="parse" {
            for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return;}}
            this.pending=cmd("CompileModel",#{doc_id:this.good,class:"ThirdPassGood",resume_after_compile:false});
            this.phase="compiled";return;
        }
        if this.phase=="compiled" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.good});
            this.entity=find("ThirdPassGood");
            let valid=result.ok==true && sample.compiled && sample.paused && this.entity>=0;
            this.checks.push(t_true(valid && sample.parameters.gain==6.0 && sample.variables.value==7.0,
                "CompileModel completes only after the checked selected instance is installed"));
            if !valid {throw("initial compile failed: "+result);}
            this.before=query("GetDocumentSource",#{doc_id:this.good});
            this.session=get(this.entity,"ModelicaModel.session_id");
            this.pending=cmd("CompileModel",#{doc_id:this.good,class:"MissingClass"});
            this.phase="invalid-class";return;
        }
        if this.phase=="invalid-class" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let rejected=result.ok==false && result.error!=() && result.error.contains("MissingClass");
            let preserved=get(this.entity,"ModelicaModel.session_id")==this.session
                && query("GetDocumentSource",#{doc_id:this.good}).source==this.before.source;
            this.checks.push(t_true(rejected && preserved && query("SnapshotVariables",#{doc_id:this.good}).compiled,
                "invalid compile selection returns terminal rejection without replacing the installed instance"));
            this.pending=cmd("CompileModel",#{doc_id:this.bad,class:"ThirdPassBad"});
            this.phase="bad-compile";return;
        }
        if this.phase=="bad-compile" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            this.bad_entity=find("ThirdPassBad");
            this.bad_session=get(this.bad_entity,"ModelicaModel.session_id");
            this.bad_before=query("GetDocumentSource",#{doc_id:this.bad});
            this.bad_values=query("SnapshotVariables",#{doc_id:this.bad});
            this.checks.push(t_true(result.ok==false && result.error!=() && result.error.contains("MissingValue")
                && !this.bad_values.compiled,"failed first compilation is terminal and retains an uncompiled document actor"));
            this.pending=cmd("ResetActiveModel",#{doc_id:this.bad});
            this.phase="bad-reset";return;
        }
        if this.phase=="bad-reset" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let rejected=result.ok==false && result.error!=() && result.error.contains("compiled");
            let unchanged=get(this.bad_entity,"ModelicaModel.session_id")==this.bad_session
                && query("GetDocumentSource",#{doc_id:this.bad}).source==this.bad_before.source;
            let values=query("SnapshotVariables",#{doc_id:this.bad});
            this.checks.push(t_true(rejected && unchanged && values.variables==this.bad_values.variables,
                "Reset rejects a failed first compile before changing session identity or observations"));
            this.pending=cmd("ResetActiveModel",#{doc_id:this.good});
            this.reset_session=get(this.entity,"ModelicaModel.session_id");
            let sample=query("SnapshotVariables",#{doc_id:this.good});
            let pending=this.pending.status=="pending" && query("CompileStatus",#{doc_id:this.good}).is_compiling;
            this.checks.push(t_true(pending && sample.variables.value==7.0 && this.reset_session>this.session,
                "accepted Reset stays pending and retains installed observations until its exact worker result"));
            let busy=cmd("ResetActiveModel",#{doc_id:this.good});
            let busy_rejected=busy.ok==false && busy.error!=() && busy.error.contains("pending");
            this.checks.push(t_true(busy_rejected && get(this.entity,"ModelicaModel.session_id")==this.reset_session,
                "a second Reset is terminally rejected while the admitted reset owns the participant"));
            this.phase="reset";return;
        }
        if this.phase=="reset" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.good});
            let installed=sample.compiled && sample.parameters.gain==6.0 && sample.variables.value==7.0;
            let exact=get(this.entity,"ModelicaModel.session_id")==this.reset_session
                && !query("CompileStatus",#{doc_id:this.good}).is_compiling;
            this.checks.push(t_true(result.ok==true && installed && exact,
                "Reset completes after installing its exact checked session and initialized values"));
            this.before=query("GetDocumentSource",#{doc_id:this.good});
            this.pending=cmd("SaveActiveDocumentAs",#{doc_id:this.good,path:this.invalid_save_path});
            this.phase="save-failed";return;
        }
        if this.phase=="save-failed" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let source=query("GetDocumentSource",#{doc_id:this.good});
            let rejected=result.ok==false && result.error!=() && result.error.contains(this.invalid_save_path);
            let unchanged=source.source==this.before.source && source.generation==this.before.generation
                && source.origin==this.before.origin;
            this.checks.push(t_true(rejected && unchanged,
                "active Save As preserves the original terminal failure and leaves source identity unchanged"));
            this.pending=cmd("SaveActiveDocumentAs",#{doc_id:this.good,path:this.save_path});
            this.phase="save-as";return;
        }
        if this.phase=="save-as" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let source=query("GetDocumentSource",#{doc_id:this.good});
            this.saved=source.origin.kind=="file";
            let identity=source.origin.path==this.save_path && source.dirty==false;
            let unchanged=source.source==this.before.source && source.generation==this.before.generation;
            this.checks.push(t_true(result.ok==true && identity && unchanged,
                "active Save As completes the original receipt after writing the exact admitted document"));
            this.edited=source.source;
            this.edited.replace("parameter Real gain=2*3;","parameter Real gain=9;");
            let edited=editor::replace_source(this.good,this.edited,source.generation);
            if edited.ok!=true {throw("source edit failed: "+edited);}
            this.pending=cmd("SaveActiveDocument",#{doc_id:this.good});
            this.phase="save";return;
        }
        if this.phase=="save" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let source=query("GetDocumentSource",#{doc_id:this.good});
            let exact=source.source==this.edited && source.origin.path==this.save_path && source.dirty==false;
            this.checks.push(t_true(result.ok==true && exact && get(this.entity,"ModelicaModel.session_id")==this.reset_session,
                "active Save completes through the same owner without changing the live simulation session"));
            editor::close_scratch(this.to_retire);this.retired=this.to_retire;
            this.pending=cmd("SaveActiveDocumentAs",#{doc_id:this.retired,path:this.save_path});
            this.phase="retired";return;
        }
        if this.phase=="retired" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            this.checks.push(t_true(result.ok==false && result.error!=(),
                "active save rejects a retired document with a terminal diagnostic"));
            this.phase="interleave-ready";return;
        }
        if this.phase=="interleave-ready" {
            if query("CompileStatus",#{doc_id:this.good}).ast_parsed!=true {return;}
            this.reset=cmd("ResetActiveModel",#{doc_id:this.good});
            this.pending=cmd("CompileModel",#{doc_id:this.good,class:"ThirdPassGood",force:true,resume_after_compile:false});
            this.phase="interleaved";return;
        }
        if this.phase=="interleaved" {
            let compiled=terminal(this.pending);let reset=terminal(this.reset);
            if compiled.status=="pending" || reset.status=="pending" {return;}
            let reset_terminal=reset.ok==true;
            if reset.ok==false && reset.error!=() {
                reset_terminal=!reset.error.is_empty();
            }
            let sample=query("SnapshotVariables",#{doc_id:this.good});
            let current=sample.compiled && sample.model_name=="ThirdPassGood" && sample.parameters.gain==9.0;
            this.checks.push(t_true(compiled.ok==true && reset_terminal && current && sample.variables.value==10.0,
                "compile and Reset interleaving terminates both receipts and publishes only the newest checked source"));
            this=finish(this,this.checks);
        }
    } catch(error) {this=finish(this,["third-pass lifecycle failed at "+this.phase+": "+error]);}
}
