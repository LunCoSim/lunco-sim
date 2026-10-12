// Retained trajectories carry the checked facts from their own compile.
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult", "GetDocumentSource"], required_inputs: []}
}
fn source(unit) {
    `model RunMetadataProbe
  type Distance=Real(unit="m");
  model Hot Real sample(unit="K") "hot reading"; equation sample=1; end Hot;
  model Cold Real sample(unit="` + unit + `") "cold reading"; equation sample=2; end Cold;
  model PlainPin Real v; equation v=3; end PlainPin;
  Hot hot; Cold cold; PlainPin p;
  Distance vector[2](start={1,2},fixed=true) "vector distance";
  equation der(vector)={1,2};
end RunMetadataProbe;`
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn facts(id, cold_unit) {
    let result=query("GetExperimentResult", #{experiment_id:id});
    let metadata=if result != () { result.metadata } else { () };
    if metadata == () { return ["retained run has no checked metadata"]; }
    for name in ["hot.sample", "cold.sample", "vector[1]", "vector[2]", "p.v"] {
        if metadata[name] == () { return ["checked run omitted metadata for " + name]; }
    }
    let filtered=query("GetExperimentResult", #{experiment_id:id,variables:["vector[2]"]});
    [t_true(metadata["hot.sample"].unit=="K" && metadata["hot.sample"].description=="hot reading",
       "qualified hot facts belong to the run"),
     t_true(metadata["cold.sample"].unit==cold_unit && metadata["cold.sample"].description=="cold reading",
       "repeated leaf names preserve distinct checked facts"),
     t_true(metadata["vector[1]"].unit=="m" && metadata["vector[2]"].unit=="m"
         && metadata["vector[2]"].description=="vector distance",
       "scalar array series retain inherited units"),
     t_true(metadata["p.v"].unit==() && metadata["p.v"].description==(),
       "connector-shaped series do not fabricate units"),
     t_true(filtered != () && filtered.metadata != () && filtered.metadata.len()==1 && filtered.metadata["vector[2]"] != () && filtered.metadata["vector[2]"].unit=="m",
       "result facts use the requested series filter")]
}
fn finish(state, failures) {
    if state.done { return state; }
    state.done=true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures,"MODELICA RUN METADATA","MODELICA_RUN_METADATA"); state
}
fn on_start(me, ctx) {
    this.done=false; this.doc=(); this.ticks=0; this.phase="ready";
    this.run=(); this.previous=(); this.failures=[];
    let opened=editor::new_scratch(source("s"),"Run metadata facts");
    if opened==() || opened.ok!=true { this=finish(this,["scratch source admission failed"]); return; }
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>6000 { this=finish(this,["run metadata timed out at "+this.phase]); return; }
    if this.phase=="ready" || this.phase=="successor_ready" {
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status==() || status.ast_parsed!=true || !status.candidates.contains("RunMetadataProbe") { return; }
        this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"RunMetadataProbe",t_end:0.1,n_intervals:10});
        this.phase="admission"; return;
    }
    if this.phase=="admission" {
        let admitted=ack(this.pending);
        if admitted==() || admitted.status=="pending" { return; }
        if admitted.ok!=true || admitted.data==() || admitted.data.experiment_id==() {
            this=finish(this,["run admission failed: "+admitted.to_string()]); return;
        }
        this.run=admitted.data.experiment_id; this.phase="complete";
    }
    if this.phase!="complete" { return; }
    let status=query("RunStatus",#{experiment_id:this.run});
    if status==() || status.state=="queued" || status.state=="running" || status.state=="pending" { return; }
    if status.state!="done" { this=finish(this,["run failed: "+status.to_string()]); return; }
    if this.previous==() {
        this.failures+=facts(this.run,"s"); this.previous=this.run;
        let current=query("GetDocumentSource",#{doc_id:this.doc});
        let replaced=editor::replace_source(this.doc,source("kg"),current.generation);
        if replaced.ok!=true { this=finish(this,["draft replacement failed"]); return; }
        this.failures+=facts(this.previous,"s");
        this.phase="successor_ready"; return;
    }
    this.failures+=facts(this.run,"kg");
    this.failures+=facts(this.previous,"s");
    this=finish(this,this.failures);
}
