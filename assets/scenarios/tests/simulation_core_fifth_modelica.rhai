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
    report_verdict(checks,"MODELICA FIFTH PASS","MODELICA_FIFTH_PASS");
    for doc in state.docs {editor::close_scratch(doc);}
    state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",checks:[],docs:[],pending:()};
    try {
        let bad=editor::new_scratch("model FifthRunBad Real x; equation x=MissingFifth; end FifthRunBad;","Fifth invalid Run");
        let choices=editor::new_scratch(`model FifthA
          Real x(start=1,fixed=true); equation der(x)=1;
        end FifthA;
        model FifthB
          Real x(start=2,fixed=true); equation der(x)=2;
        end FifthB;`,"Fifth ordered targets");
        for created in [bad,choices] {
            if created.ok!=true {throw("scratch creation failed: "+created);}
            this.docs.push(created.doc_id);
        }
        this.bad=bad.doc_id;this.choices=choices.doc_id;
    } catch(error) {this=finish(this,["fifth-pass startup failed: "+error]);}
}
fn on_tick(me, ctx) {
    if this.done {return;}
    try {
        this.elapsed+=execution_context().delta_seconds;
        if this.elapsed>90.0 {throw("timeout at "+this.phase+"; pending="+terminal(this.pending));}
        if this.phase=="parse" {
            for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return;}}
            this.pending=cmd("RunActiveModel",#{doc_id:this.choices});
            this.phase="ambiguous";return;
        }
        if this.phase=="run-failed" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let status=query("CompileStatus",#{doc_id:this.bad});
            let failed=result.ok==false && result.error!=() && result.error.contains("MissingFifth");
            this.checks.push(t_true(failed && !status.is_compiled,
                "Run returns its delegated compiler failure instead of terminal success: "+result));
            this=finish(this,this.checks);return;
        }
        if this.phase=="ambiguous" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let rejected=result.ok==false && result.error!=() && result.error.contains("explicit class");
            this.checks.push(t_true(rejected && !query("CompileStatus",#{doc_id:this.choices}).is_compiled,
                "external ambiguous Run rejects terminally without requiring an interactive picker"));
            this.earlier=cmd("CompileModel",#{doc_id:this.choices,class:"FifthB",force:true});
            this.pending=cmd("CompileModel",#{doc_id:this.choices,class:"FifthA",force:true});
            this.phase="b-then-a";return;
        }
        if this.phase=="b-then-a" {
            let earlier=terminal(this.earlier);let later=terminal(this.pending);
            if earlier.status=="pending" || later.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            let selected=sample.compiled && sample.model_name=="FifthA" && sample.variables.x==1.0;
            this.checks.push(t_true(earlier.ok==false && later.ok==true && selected,
                "ordered Compile(B,A) installs the later A and terminally supersedes B: "+sample));
            this.earlier=cmd("CompileModel",#{doc_id:this.choices,class:"FifthA",force:true});
            this.pending=cmd("CompileModel",#{doc_id:this.choices,class:"FifthB",force:true});
            this.phase="a-then-b";return;
        }
        if this.phase=="a-then-b" {
            let earlier=terminal(this.earlier);let later=terminal(this.pending);
            if earlier.status=="pending" || later.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            let selected=sample.compiled && sample.model_name=="FifthB" && sample.variables.x==2.0;
            this.checks.push(t_true(earlier.ok==false && later.ok==true && selected,
                "ordered Compile(A,B) also installs the later explicit selection"));
            this.entity=find("FifthB");
            this.session=get(this.entity,"ModelicaModel.session_id");
            this.earlier=cmd("CompileModel",#{doc_id:this.choices,class:"FifthB",force:true});
            this.pending=cmd("CompileModel",#{doc_id:this.choices,class:"FifthB",force:true});
            this.phase="coalesced";return;
        }
        if this.phase=="coalesced" {
            let earlier=terminal(this.earlier);let later=terminal(this.pending);
            if earlier.status=="pending" || later.status=="pending" {return;}
            let single=parse_int(get(this.entity,"ModelicaModel.session_id").to_string())==parse_int(this.session.to_string())+1;
            this.checks.push(t_true(earlier.ok==true && later.ok==true && single,
                "consecutive identical compile requests share one installation and complete both listeners"));
            this.run=cmd("RunActiveModel",#{doc_id:this.choices,class:"FifthA"});
            this.pending=cmd("PauseActiveModel",#{doc_id:this.choices});
            this.phase="run-paused";return;
        }
        if this.phase=="run-paused" {
            let run=terminal(this.run);let pause=terminal(this.pending);
            if run.status=="pending" || pause.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            let installed=sample.compiled && sample.model_name=="FifthA" && sample.t==0.0;
            this.checks.push(t_true(run.ok==true && pause.ok==true && installed && sample.paused,
                "successful Run completes its selected installation while preserving a later Pause"));
            this.entity=find("FifthA");this.session=get(this.entity,"ModelicaModel.session_id");
            this.pending=cmd("RunActiveModel",#{doc_id:this.choices,class:"MissingFifthClass"});
            this.phase="invalid-selection";return;
        }
        if this.phase=="invalid-selection" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let preserved=get(this.entity,"ModelicaModel.session_id")==this.session;
            this.checks.push(t_true(result.ok==false && result.error.contains("MissingFifthClass") && preserved,
                "invalid explicit Run selection rejects without replacing the installed session"));
            this.pending=cmd("RunActiveModel",#{doc_id:this.choices,class:"FifthA"});
            this.phase="running";return;
        }
        if this.phase=="running" {
            let result=terminal(this.pending);if result.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            if sample.t<=0.0 && result.ok==true {return;}
            if result.ok!=true {throw("valid fast Run rejected: "+result);}
            this.pending=cmd("PauseActiveModel",#{doc_id:this.choices});
            this.quiet_time=sample.t;this.quiet_frames=0;
            this.phase="drain-pause";return;
        }
        if this.phase=="drain-pause" {
            let pause=terminal(this.pending);if pause.status=="pending" {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            if sample.t!=this.quiet_time {this.quiet_time=sample.t;this.quiet_frames=0;return;}
            this.quiet_frames+=1;if this.quiet_frames<3 {return;}
            this.checks.push(t_true(pause.ok==true && sample.paused && sample.compiled,
                "Pause retains and drains any admitted endpoint before the model remains frozen"));
            this.frozen=sample.t;this.tick=clock_snapshot().sim_tick;
            this.phase="frozen";return;
        }
        if this.phase=="frozen" {
            if clock_snapshot().sim_tick<=this.tick+2 {return;}
            let sample=query("SnapshotVariables",#{doc_id:this.choices});
            this.checks.push(t_true(sample.paused && sample.t==this.frozen,
                "the shared clock continues after the paused participant has completed its boundary"));
            this.pending=cmd("RunActiveModel",#{doc_id:this.bad,class:"FifthRunBad"});
            this.checks.push(t_true(this.pending.status=="pending",
                "Run retains its original receipt while checked compilation is outstanding"));
            this.phase="run-failed";
        }
    } catch(error) {this.checks.push("fifth-pass failed at "+this.phase+": "+error);this=finish(this,this.checks);}
}
