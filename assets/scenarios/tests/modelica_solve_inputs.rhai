// Numeric input admission and integration through the UI-free solve command.
// @timing presentation
const TEST_KIND = "editor";
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities:[],entity_reads:[],entity_writes:[],
      query_reads:["CompileStatus","GetDocumentSource","RunStatus","GetExperimentResult"],required_inputs:[]}
}
fn ack(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn finish(state, checks) {
    state.done=true;editor::close_scratch(state.doc);
    report_verdict(checks,"MODELICA SOLVE INPUTS","MODELICA_SOLVE_INPUTS");state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",checks:[]};
    let opened=editor::new_scratch(`model HeldInputContract
      input Real forcing;
      output Real integral(start=0,fixed=true);
    equation der(integral)=forcing;
    end HeldInputContract;`,"Held input contract");
    if opened.ok!=true {throw("input contract document was not created");}
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done {return;}
    this.elapsed+=execution_context().delta_seconds;
    if this.elapsed>30.0 {this=finish(this,["input contract timed out at "+this.phase]);return;}
    if this.phase=="parse" {
        if query("CompileStatus",#{doc_id:this.doc}).ast_parsed!=true {return;}
        this.spec=#{doc_id:this.doc,class:"HeldInputContract",
            source_generation:query("GetDocumentSource",#{doc_id:this.doc}).generation,
            inputs:#{forcing:4.0},t_start:0.0,t_end:10.0,dt:1.0,tolerance:1.0e-9,label:"Held Real input"};
        let invalid=this.spec;invalid.inputs=#{"":4.0};
        this.pending=cmd("RunModelicaSolve",invalid);this.phase="name";return;
    }
    if this.phase=="name" || this.phase=="generation" || this.phase=="tolerance" {
        let result=ack(this.pending);if result==() || result.status=="pending" {return;}
        if this.phase=="name" {
            this.checks.push(t_true(result.ok==false && result.error.contains("non-empty names"),
                "empty input identity rejects before dispatch"));
            let invalid=this.spec;invalid.source_generation=this.spec.source_generation.to_int()+1;
            this.pending=cmd("RunModelicaSolve",invalid);this.phase="generation";return;
        }
        if this.phase=="generation" {
            this.checks.push(t_true(result.ok==false && result.error.contains("stale source generation"),
                "stale source generation rejects before dispatch"));
            let invalid=this.spec;invalid.tolerance=0.0;
            this.pending=cmd("RunModelicaSolve",invalid);this.phase="tolerance";return;
        }
        this.checks.push(t_true(result.ok==false,"invalid tolerance rejects before dispatch"));
        this.pending=cmd("RunModelicaSolve",this.spec);this.phase="dispatch";return;
    }
    if this.phase=="dispatch" {
        let result=ack(this.pending);if result==() || result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["valid input solve rejected: "+result]);return;}
        this.run=result.data.experiment_id;this.phase="wait";
    }
    if this.phase=="wait" {
        let status=query("RunStatus",#{experiment_id:this.run});
        if status.state=="failed" || status.state=="cancelled" {this=finish(this,["input solve failed: "+status]);return;}
        if status.state!="done" {return;}
        let result=query("GetExperimentResult",#{experiment_id:this.run,variables:["integral"],max_points:64});
        this.checks.push(t_true(status.doc_id==this.doc,"the result retains its source document identity"));
        let complete=!result.downsampled && result.missing_variables.is_empty()
            && result.times.len()==11 && result.times[0]==0.0 && result.times[10]==10.0;
        this.checks.push(t_true(complete,"the full requested output grid is returned"));
        let accurate=complete && result.series.integral.len()==11;
        for row in 0..result.times.len() {
            accurate=accurate && result.series.integral[row].is_finite()
                && abs(result.series.integral[row]-4.0*result.times[row])<1.0e-8;
        }
        this.checks.push(t_true(accurate,"the held input drives the actual integrated state"));
        this=finish(this,this.checks);
    }
}
