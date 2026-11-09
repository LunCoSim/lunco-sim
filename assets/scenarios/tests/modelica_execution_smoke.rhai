// Execute inline Modelica equations without requiring an optional library.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn finish(state, failures) {
    state.done=true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures,"MODELICA EXECUTION SMOKE","MODELICA_EXECUTION_SMOKE"); state
}
fn on_start(me, ctx) {
    this=#{done:false,doc:(),ticks:0,phase:"parse",index:0,failures:[],
      classes:["StaticOutputProbe","DynamicResponseProbe","MissingDeclarationProbe"]};
    let opened=editor::new_scratch(`model StaticOutputProbe
  output Real voltage(unit="V");
  output Real current(unit="A");
equation voltage=5; current=voltage/10;
end StaticOutputProbe;
model DynamicResponseProbe
  input Real command=2;
  output Real response(start=0,fixed=true);
equation der(response)=command-response;
end DynamicResponseProbe;
model MissingDeclarationProbe
  UndefinedComponent missing;
end MissingDeclarationProbe;`,"Inline Modelica execution contract");
    if opened==() || opened.ok!=true { this=finish(this,["scratch admission failed"]); return; }
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>18000 { this=finish(this,["Modelica execution timed out at "+this.phase]); return; }
    if this.phase=="parse" {
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status==() || status.ast_parsed!=true { return; }
        this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:this.classes[this.index],t_end:1.0,n_intervals:10,tolerance:1.0e-8});
        this.phase="admission"; return;
    }
    if this.phase=="admission" {
        let admitted=this.pending;
        if admitted!=() && admitted.status=="pending" && admitted.id!=() { admitted=command_result(admitted.id); }
        if admitted==() || admitted.status=="pending" { return; }
        if admitted.ok!=true {
            if this.index==2 {
                this.failures.push(t_true(admitted.ok==false,"missing class rejects command admission"));
                this=finish(this,this.failures);
            } else { this=finish(this,["Modelica run admission failed: "+admitted.to_string()]); }
            return;
        }
        this.run=admitted.data.experiment_id; this.phase="complete";
    }
    if this.phase!="complete" { return; }
    let status=query("RunStatus",#{experiment_id:this.run});
    if status==() || status.state=="pending" || status.state=="queued" || status.state=="running" { return; }
    if this.index==2 {
        this.failures.push(t_true(status.state=="failed","missing class terminates compilation"));
        this=finish(this,this.failures); return;
    }
    if status.state!="done" { this=finish(this,["Modelica solve failed: "+status.to_string()]); return; }
    let result=query("GetExperimentResult",#{experiment_id:this.run});
    if this.index==0 {
        let valid=result!=() && result.series!=() && result.series.voltage!=() && result.series.current!=()
            && result.series.voltage.len()==11 && result.series.current.len()==11;
        if valid {
            for value in result.series.voltage { valid=valid && (value-5.0).abs()<1.0e-8; }
            for value in result.series.current { valid=valid && (value-0.5).abs()<1.0e-8; }
        }
        this.failures.push(t_true(valid,"constant equations produce the requested voltage and current"));
        this.failures.push(t_true(result!=() && result.metadata!=() && result.metadata.voltage!=() && result.metadata.current!=()
            && result.metadata.voltage.unit=="V" && result.metadata.current.unit=="A","output trajectory retains authored units"));
    } else {
        let valid=result!=() && result.series!=() && result.series.response!=() && result.times!=()
            && result.times.len()==11 && result.series.response.len()==11;
        if valid {
            for i in 0..result.times.len() { valid=valid && (result.series.response[i]-2.0*(1.0-(-result.times[i]).exp())).abs()<1.0e-5; }
        }
        this.failures.push(t_true(valid,"continuous state follows its analytic trajectory"));
    }
    this.index+=1;
    if this.index<3 { this.phase="parse"; return; }
    this=finish(this,this.failures);
}
