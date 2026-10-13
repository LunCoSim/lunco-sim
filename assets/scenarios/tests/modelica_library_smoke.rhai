// Execute maintained library declarations through checked production admission.
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn finish(state, failures) {
    state.done=true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures,"MODELICA LIBRARY SMOKE","MODELICA_LIBRARY_SMOKE"); state
}
fn on_start(me, ctx) {
    this=#{done:false,doc:(),ticks:0,phase:"parse",index:0,failures:[],
      classes:["LibraryCircuitProbe","LibraryFilterProbe","LibraryMissingProbe"]};
    let opened=editor::new_scratch(`model LibraryCircuitProbe
  Modelica.Electrical.Analog.Basic.Resistor resistor(R=10);
  Modelica.Electrical.Analog.Sources.ConstantVoltage supply(V=5);
  Modelica.Electrical.Analog.Basic.Ground ground;
  output Real voltage(unit="V")=resistor.v;
  output Real current(unit="A")=resistor.i;
equation
  connect(supply.p,resistor.p); connect(resistor.n,supply.n); connect(supply.n,ground.p);
end LibraryCircuitProbe;
model LibraryFilterProbe
  Modelica.Blocks.Continuous.FirstOrder filter(T=1,initType=Modelica.Blocks.Types.Init.InitialState,y_start=0);
  Modelica.Blocks.Sources.Constant command(k=2);
  output Real response=filter.y;
equation connect(command.y,filter.u);
end LibraryFilterProbe;
model LibraryMissingProbe
  Modelica.Electrical.Analog.Basic.MissingComponent missing;
end LibraryMissingProbe;`,"Library execution contract");
    if opened==() || opened.ok!=true { this=finish(this,["scratch admission failed"]); return; }
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>18000 { this=finish(this,["library execution timed out at "+this.phase]); return; }
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
            if this.index==2 { this.failures.push(t_true(admitted.ok==false,"missing library class rejects command admission")); this=finish(this,this.failures); }
            else { this=finish(this,["library admission failed: "+admitted.to_string()]); }
            return;
        }
        this.run=admitted.data.experiment_id; this.phase="complete";
    }
    if this.phase!="complete" { return; }
    let status=query("RunStatus",#{experiment_id:this.run});
    if status==() || status.state=="pending" || status.state=="queued" || status.state=="running" { return; }
    if this.index==2 {
        this.failures.push(t_true(status.state=="failed","missing library class terminates compilation"));
        this=finish(this,this.failures); return;
    }
    if status.state!="done" { this=finish(this,["library solve failed: "+status.to_string()]); return; }
    let result=query("GetExperimentResult",#{experiment_id:this.run});
    if this.index==0 {
        let valid=result!=() && result.series!=() && result.series.voltage!=() && result.series.current!=()
            && result.series.voltage.len()==11 && result.series.current.len()==11;
        if valid {
            for value in result.series.voltage { valid=valid && (value-5.0).abs()<1.0e-8; }
            for value in result.series.current { valid=valid && (value-0.5).abs()<1.0e-8; }
        }
        this.failures.push(t_true(valid,"acausal library circuit satisfies its voltage and current equations"));
        this.failures.push(t_true(result!=() && result.metadata!=() && result.metadata.voltage!=() && result.metadata.current!=()
            && result.metadata.voltage.unit=="V" && result.metadata.current.unit=="A","checked library trajectory retains electrical units"));
    } else {
        let valid=result!=() && result.series!=() && result.series.response!=() && result.times!=()
            && result.times.len()==11 && result.series.response.len()==11;
        if valid {
            for i in 0..result.times.len() { valid=valid && (result.series.response[i]-2.0*(1.0-(-result.times[i]).exp())).abs()<1.0e-5; }
        }
        this.failures.push(t_true(valid,"dynamic library block follows its analytic trajectory"));
    }
    this.index+=1; this.phase="parse";
}
