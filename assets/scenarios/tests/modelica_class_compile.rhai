// Qualified and quoted source identities reach the production batch compiler.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn finish(state, failures) {
    state.done=true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures,"MODELICA CLASS COMPILE","MODELICA_CLASS_COMPILE"); state
}
fn on_start(me, ctx) {
    this=#{done:false, ticks:0, doc:(), phase:"parse", target:0, failures:[]};
    let opened=editor::new_scratch(`package CompileIdentity
  model Plain output Real answer(unit="K")=1; end Plain;
  model 'Quoted.Name' output Real answer(unit="m")=2; end 'Quoted.Name';
end CompileIdentity;`,"Class compile identity");
    if opened.ok!=true { this=finish(this,["scratch source admission failed"]); return; }
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>6000 { this=finish(this,["class compile timed out at "+this.phase]); return; }
    if this.phase=="parse" {
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status == () || status.ast_parsed!=true { return; }
        this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"Other.CompileIdentity.Plain",t_end:0.02,n_intervals:2});
        this.phase="reject"; return;
    }
    if this.phase=="reject" {
        let rejected=ack(this.pending);
        if rejected==() || rejected.status=="pending" { return; }
        this.failures.push(t_true(rejected.ok==false,"unrelated package is rejected before compilation"));
        this.phase="start";
    }
    if this.phase=="start" {
        let target=if this.target==0 { "CompileIdentity.Plain" } else { "CompileIdentity.'Quoted.Name'" };
        this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:target,t_end:0.02,n_intervals:2});
        this.phase="admission"; return;
    }
    if this.phase=="admission" {
        let admitted=ack(this.pending);
        if admitted==() || admitted.status=="pending" { return; }
        if admitted.ok!=true || admitted.data==() || admitted.data.experiment_id==() {
            this=finish(this,["qualified run admission failed: "+admitted.to_string()]); return;
        }
        this.run=admitted.data.experiment_id; this.phase="complete";
    }
    if this.phase!="complete" { return; }
    let status=query("RunStatus",#{experiment_id:this.run});
    if status==() || status.state=="queued" || status.state=="running" || status.state=="pending" { return; }
    if status.state!="done" { this=finish(this,["qualified run failed: "+status.to_string()]); return; }
    let result=query("GetExperimentResult",#{experiment_id:this.run,variables:["answer"]});
    let expected=if this.target==0 { 1.0 } else { 2.0 };
    let unit=if this.target==0 { "K" } else { "m" };
    if result==() || result.series.answer==() || result.metadata.answer==() {
        this=finish(this,["qualified run omitted its output facts"]); return;
    }
    let correct=true;
    for sample in result.series.answer { if sample!=expected { correct=false; } }
    this.failures.push(t_true(result.series.answer.len()==3 && correct
        && result.metadata.answer.unit==unit,"compiled target retains its own trajectory and metadata"));
    if this.target==0 { this.target=1; this.phase="start"; return; }
    this=finish(this,this.failures);
}
