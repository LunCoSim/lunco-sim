// Parsed document scope selects exact targets in both production run APIs.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "GetDocumentSource", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn finish(state, failures) {
    state.done=true;
    for doc in state.docs { editor::close_scratch(doc); }
    report_verdict(failures,"MODELICA CLASS COMPILE","MODELICA_CLASS_COMPILE"); state
}
fn on_start(me, ctx) {
    this=#{done:false, ticks:0, docs:[], phase:"parse", target:0, failures:[]};
    let opened=editor::new_scratch(`within RunSpace;
package CompileIdentity
  model Plain
    import SI = Modelica.Units.SI;
    output SI.Temperature answer=1;
  end Plain;
  model 'Quoted.Name' output Real answer(unit="m")=2; end 'Quoted.Name';
end CompileIdentity;`,"Class compile identity");
    if opened.ok!=true { this=finish(this,["scratch source admission failed"]); return; }
    this.doc=opened.doc_id; this.docs.push(this.doc);
    let default_doc=editor::new_scratch(`within RunSpace;
model DefaultRoot output Real answer(unit="K")=3; end DefaultRoot;`,"Default run identity");
    if default_doc.ok!=true { this=finish(this,["default source admission failed"]); return; }
    this.default_doc=default_doc.doc_id; this.docs.push(this.default_doc);
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>6000 { this=finish(this,["class compile timed out at "+this.phase]); return; }
    if this.phase=="parse" {
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status == () || status.ast_parsed!=true { return; }
        let default_status=query("CompileStatus",#{doc_id:this.default_doc});
        if default_status==() || default_status.ast_parsed!=true { return; }
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        this.pending=editor::apply(this.doc,[
            modelica_add_component("RunSpace.CompileIdentity.Plain","SI.Temperature","ambient",
                modelica_placement(0.0,0.0,20.0,20.0),[]),
            modelica_add_equation("RunSpace.CompileIdentity.Plain","ambient","2.0")
        ],source.generation);
        this.phase="edit"; return;
    }
    if this.phase=="edit" {
        let edited=ack(this.pending);
        if edited==() || edited.status=="pending" { return; }
        if edited.ok!=true { this=finish(this,["imported component edit rejected: "+edited.to_string()]); return; }
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status==() || status.ast_parsed!=true { return; }
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
        if this.target==0 {
            this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"RunSpace.CompileIdentity.Plain",t_end:0.02,n_intervals:2});
        } else if this.target==1 {
            this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"CompileIdentity.'Quoted.Name'",t_end:0.02,n_intervals:2});
        } else if this.target==2 {
            this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"'Quoted.Name'",t_end:0.02,n_intervals:2});
        } else if this.target==3 {
            let source=query("GetDocumentSource",#{doc_id:this.doc});
            this.pending=cmd("RunModelicaSolve",#{doc_id:this.doc,class:"CompileIdentity.Plain",
                source_generation:source.generation,t_start:0.0,t_end:0.02,dt:0.01,label:"Headless run identity"});
        } else {
            this.pending=cmd("RunExperiment",#{doc_id:this.default_doc,t_end:0.02,n_intervals:2});
        }
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
    let result=query("GetExperimentResult",#{experiment_id:this.run,variables:["answer","ambient"]});
    let expected=if this.target==0 || this.target==3 { 1.0 } else if this.target==4 { 3.0 } else { 2.0 };
    let unit=if this.target==1 || this.target==2 { "m" } else { "K" };
    if result==() || result.series.answer==() || result.metadata.answer==() {
        this=finish(this,["qualified run omitted its output facts"]); return;
    }
    let correct=true;
    for sample in result.series.answer { if sample!=expected { correct=false; } }
    this.failures.push(t_true(result.series.answer.len()==3 && correct
        && result.metadata.answer.unit==unit,"run target "+this.target+" retains its own trajectory and metadata"));
    if this.target==0 || this.target==3 {
        let ambient_ok=result.series.ambient!=() && result.series.ambient.len()==3
            && result.metadata.ambient!=() && result.metadata.ambient.unit=="K";
        if ambient_ok { for sample in result.series.ambient { ambient_ok=ambient_ok && sample==2.0; } }
        this.failures.push(t_true(ambient_ok,"structurally authored imported type keeps resolved units and values"));
    }
    if this.target<4 { this.target+=1; this.phase="start"; return; }
    this=finish(this,this.failures);
}
