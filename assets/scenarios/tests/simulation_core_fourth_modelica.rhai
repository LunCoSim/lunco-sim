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
fn near(value, expected) {value!=() && value.is_finite() && (value-expected).abs()<1.0e-9}
fn finish(state, checks) {
    state.done=true;
    report_verdict(checks,"MODELICA FOURTH PASS","MODELICA_FOURTH_PASS");
    for doc in state.docs {editor::close_scratch(doc);}
    state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",checks:[],docs:[],pending:()};
    try {
        let seed=editor::new_scratch(`model FourthResetSeed
          input Real u=1; Real x(fixed=false); output Real y;
          initial equation x=u;
          equation der(x)=0; y=x;
        end FourthResetSeed;`,"Fourth reset seed");
        let control=editor::new_scratch(`model FourthRunIntent
          Real x(start=1,fixed=true); equation der(x)=1;
        end FourthRunIntent;`,"Fourth run intent");
        let primary=editor::new_scratch(`model FourthPrimary
          FourthHelper h; output Real y; equation y=h.gain;
        end FourthPrimary;`,"Fourth primary");
        let sibling=editor::new_scratch("model FourthSiblingOld parameter Real gain=99; end FourthSiblingOld;","Fourth sibling");
        let empty=editor::new_scratch("model FourthUncompiled Real x; equation der(x)=0; end FourthUncompiled;","Fourth uncompiled");
        for created in [seed,control,primary,sibling,empty] {
            if created.ok!=true {throw("scratch creation failed: "+created);}
            this.docs.push(created.doc_id);
        }
        this.seed=seed.doc_id;this.control=control.doc_id;this.primary=primary.doc_id;
        this.sibling=sibling.doc_id;this.uncompiled=empty.doc_id;
    } catch(error) {this=finish(this,["fourth-pass startup failed: "+error]);}
}
fn on_tick(me, ctx) {
    if this.done {return;}
    try {
        this.elapsed+=execution_context().delta_seconds;
        if this.elapsed>100.0 {throw("timeout at "+this.phase+"; pending="+terminal(this.pending));}
        if this.phase=="parse" {
            for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return;}}
            this.pending=cmd("CompileModel",#{doc_id:this.seed,class:"FourthResetSeed"});
            this.phase="seed";return;
        }
        if this.phase=="seed" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            let valid=result.ok==true && sample.compiled && sample.paused;
            this.checks.push(t_true(valid && near(sample.variables.y,1.0),
                "initial equation consumes the original checked input seed"));
            if !valid {throw("initial compile failed: "+result);}
            this.seed_entity=find("FourthResetSeed");
            this.pending=cmd("SetModelInput",#{doc_id:this.seed,name:"u",value:2.0});
            this.phase="input";return;
        }
        if this.phase=="input" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            if result.ok!=true {throw("input rejected: "+result);}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            if !near(sample.inputs.u,2.0) {return;}
            this.checks.push(t_true(sample.paused && near(sample.variables.y,1.0),
                "paused input edit is admitted without inventing reinitialized state"));
            this.pending=cmd("ResetActiveModel",#{doc_id:this.seed});
            this.phase="reset-new";return;
        }
        if this.phase=="reset-new" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            let correct=near(sample.inputs.u,2.0) && near(sample.variables.x,2.0) && near(sample.variables.y,2.0);
            this.checks.push(t_true(result.ok==true && correct && sample.paused && sample.t==0.0,
                "Reset prepares the retained checked DAE with the changed input before initialization: "+sample));
            this.pending=cmd("ResetActiveModel",#{doc_id:this.seed});
            this.phase="reset-cached";return;
        }
        if this.phase=="reset-cached" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            this.checks.push(t_true(result.ok==true && near(sample.variables.y,2.0) && sample.t==0.0,
                "identical-input Reset reuses the checked preparation identity without changing initial state"));
            this.run=cmd("RunActiveModel",#{doc_id:this.control,class:"FourthRunIntent"});
            this.pending=cmd("PauseActiveModel",#{doc_id:this.control});
            this.phase="run-pause";return;
        }
        if this.phase=="run-pause" {
            let run=terminal(this.run);let pause=terminal(this.pending);
            if run.status=="pending" || pause.status=="pending" {return;}
            let status=query("CompileStatus",#{doc_id:this.control});
            if status.is_compiling {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.control});
            if !sample.compiled && status.state!="error" {return;}
            let stopped=sample.compiled && sample.paused && sample.t==0.0;
            this.checks.push(t_true(run.ok==true && pause.ok==true && stopped,
                "cold Run then Pause both complete and latest paused intent survives actor creation: "+sample));
            this.pending=cmd("ResumeActiveModel",#{doc_id:this.control});
            this.phase="resume";return;
        }
        if this.phase=="resume" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.control});
            if sample.t<=0.0 && result.ok==true {return;}
            this.checks.push(t_true(result.ok==true && !sample.paused && sample.t>0.0,
                "Resume completes terminally and advances the installed model"));
            this.pending=cmd("PauseActiveModel",#{doc_id:this.control});
            this.phase="pause";return;
        }
        if this.phase=="pause" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            this.checks.push(t_true(result.ok==true && query("SnapshotVariables",#{doc_id:this.control}).paused,
                "Pause completes terminally and freezes the selected participant"));
            this.compile=cmd("CompileModel",#{doc_id:this.control,class:"FourthRunIntent",force:true,resume_after_compile:false});
            this.pending=cmd("ResumeActiveModel",#{doc_id:this.control});
            this.phase="compile-resume";return;
        }
        if this.phase=="compile-resume" {
            let compiled=terminal(this.compile);let resumed=terminal(this.pending);
            if compiled.status=="pending" || resumed.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.control});
            if sample.t<=0.0 && compiled.ok==true {return;}
            this.checks.push(t_true(compiled.ok==true && resumed.ok==true && !sample.paused && sample.t>0.0,
                "compile-paused then Resume preserves the later running intent at checked installation"));
            cmd("PauseActiveModel",#{doc_id:this.control});
            this.session=get(this.seed_entity,"ModelicaModel.session_id");
            this.pending=cmd("RestartActiveModel",#{doc_id:this.seed});
            this.phase="restart";return;
        }
        if this.phase=="restart" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            if sample.t<=0.0 && result.ok==true {return;}
            let replaced=get(this.seed_entity,"ModelicaModel.session_id")!=this.session;
            this.checks.push(t_true(result.ok==true && replaced && !sample.paused && near(sample.variables.y,2.0),
                "Restart completes a checked replacement then runs its initialized session"));
            this.pending=cmd("PauseActiveModel",#{doc_id:this.seed});
            this.quiet_time=sample.t;this.quiet_frames=0;
            this.phase="settle";return;
        }
        if this.phase=="settle" {
            let pause=terminal(this.pending);if pause.status=="pending" {return;}
            if pause.ok!=true {throw("pause before reset failed: "+pause);}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            if sample.t!=this.quiet_time {this.quiet_frames=0;this.quiet_time=sample.t;return;}
            this.quiet_frames+=1;if this.quiet_frames<3 {return;}
            this.session=get(this.seed_entity,"ModelicaModel.session_id");
            this.restart=cmd("RestartActiveModel",#{doc_id:this.seed});
            this.pending=cmd("PauseActiveModel",#{doc_id:this.seed});
            this.phase="restart-pause";return;
        }
        if this.phase=="restart-pause" {
            let restarted=terminal(this.restart);let paused=terminal(this.pending);
            if restarted.status=="pending" || paused.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.seed});
            let replaced=get(this.seed_entity,"ModelicaModel.session_id")!=this.session;
            this.checks.push(t_true(restarted.ok==true && paused.ok==true && replaced && sample.paused && sample.t==0.0,
                "Pause after Restart admission supersedes its run intent without losing either receipt"));
            this.before=query("GetDocumentSource",#{doc_id:this.uncompiled});
            this.pending=cmd("RestartActiveModel",#{doc_id:this.uncompiled});
            this.phase="restart-invalid";return;
        }
        if this.phase=="restart-invalid" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let after=query("GetDocumentSource",#{doc_id:this.uncompiled});
            this.checks.push(t_true(result.ok==false && result.error!=() && after.source==this.before.source,
                "Restart propagates Reset admission failure without source mutation or implicit compilation"));
            editor::close_scratch(this.uncompiled);
            this.docs.pop();
            this.pending=cmd("PauseActiveModel",#{doc_id:this.uncompiled});
            this.phase="pause-invalid";return;
        }
        if this.phase=="pause-invalid" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            this.checks.push(t_true(result.ok==false && result.error!=(),
                "Pause rejects a closed document with a terminal diagnostic"));
            this.pending=cmd("ResumeActiveModel",#{doc_id:this.uncompiled});
            this.phase="resume-invalid";return;
        }
        if this.phase=="resume-invalid" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            this.checks.push(t_true(result.ok==false && result.error!=(),
                "Resume rejects a closed document with a terminal diagnostic"));
            let old=query("GetDocumentSource",#{doc_id:this.sibling});
            let edited=editor::replace_source(this.sibling,"model FourthHelper parameter Real gain=3; end FourthHelper;",old.generation);
            if edited.ok!=true {throw("sibling edit failed: "+edited);}
            this.checks.push(t_true(query("CompileStatus",#{doc_id:this.sibling}).ast_parsed==false,
                "sibling source replacement leaves an observable pending analysis generation"));
            this.pending=cmd("CompileModel",#{doc_id:this.primary,class:"FourthPrimary",force:true});
            this.phase="sibling";return;
        }
        if this.phase=="sibling" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.primary});
            this.checks.push(t_true(result.ok==true && sample.compiled && near(sample.variables.y,3.0),
                "compile awaits sibling analysis and includes its current class declarations: "+result));
            let old=query("GetDocumentSource",#{doc_id:this.sibling});
            let source="model FourthHelper parameter Real gain=MissingFourthValue; end FourthHelper;";
            let edited=editor::replace_source(this.sibling,source,old.generation);
            if edited.ok!=true {throw("negative sibling edit failed: "+edited);}
            this.invalid_source=source;
            this.pending=cmd("CompileModel",#{doc_id:this.primary,class:"FourthPrimary",force:true});
            this.phase="sibling-invalid";return;
        }
        if this.phase=="sibling-invalid" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let source=query("GetDocumentSource",#{doc_id:this.sibling});
            let rejected=result.ok==false && result.error!=() && result.error.contains("MissingFourthValue");
            this.checks.push(t_true(rejected && source.source==this.invalid_source,
                "current invalid sibling bytes reject compilation terminally instead of reusing old valid facts"));
            let edited=editor::replace_source(this.sibling,"model FourthHelper parameter Real gain=4; end FourthHelper;",source.generation);
            if edited.ok!=true {throw("sibling recovery failed: "+edited);}
            this.pending=cmd("CompileModel",#{doc_id:this.primary,class:"FourthPrimary",force:true});
            this.phase="sibling-recovered";return;
        }
        if this.phase=="sibling-recovered" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.primary});
            this.checks.push(t_true(result.ok==true && sample.compiled && near(sample.variables.y,4.0),
                "corrected sibling generation compiles and replaces the failed session without a wedge"));
            this=finish(this,this.checks);
        }
    } catch(error) {this.checks.push("fourth-pass failed at "+this.phase+": "+error);this=finish(this,this.checks);}
}
