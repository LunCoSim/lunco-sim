// Original UTF-8 text crosses document admission and the Rumoca batch compiler.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetDocumentSource", "CompileStatus", "GetDiagnostics", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn finish(state, failures) {
    state.done=true;
    for doc in state.docs { editor::close_scratch(doc); }
    report_verdict(failures,"MODELICA SOURCE ENCODING","MODELICA_SOURCE_ENCODING"); state
}
fn on_start(me, ctx) {
    this=#{done:false, ticks:0, docs:[], phase:"parse", failures:[],
      source:"﻿within Encoding;\r\nmodel 'Output.Model' \"café\"\r\n output Real answer(unit=\"K\")=3;\r\nend 'Output.Model';"};
    let opened=editor::new_scratch(this.source,"Original UTF-8 source");
    if opened.ok!=true { this=finish(this,["UTF-8 document admission failed"]); return; }
    this.doc=opened.doc_id; this.docs.push(this.doc);
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>6000 { this=finish(this,["encoding gate timed out at "+this.phase]); return; }
    if this.phase=="parse" {
        let status=query("CompileStatus",#{doc_id:this.doc});
        if status==() { return; }
        if status.error_message!=() && status.error_message!="" {
            this=finish(this,["valid UTF-8 source failed parsing: "+status.error_message]); return;
        }
        if status.ast_parsed!=true { return; }
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        this.failures.push(t_true(source.source==this.source,"document preserves leading BOM, CRLF and Unicode text"));
        this.pending=cmd("RunExperiment",#{doc_id:this.doc,class:"Encoding.'Output.Model'",t_end:0.02,n_intervals:2});
        this.phase="admission"; return;
    }
    if this.phase=="admission" {
        let admitted=ack(this.pending);
        if admitted==() || admitted.status=="pending" { return; }
        if admitted.ok!=true || admitted.data==() || admitted.data.experiment_id==() {
            this=finish(this,["UTF-8 run admission failed: "+admitted.to_string()]); return;
        }
        this.run=admitted.data.experiment_id; this.phase="complete";
    }
    if this.phase=="complete" {
        let status=query("RunStatus",#{experiment_id:this.run});
        if status==() || status.state=="queued" || status.state=="running" || status.state=="pending" { return; }
        if status.state!="done" { this=finish(this,["UTF-8 compilation failed: "+status.to_string()]); return; }
        let result=query("GetExperimentResult",#{experiment_id:this.run,variables:["answer"]});
        if result==() || result.series.answer==() || result.metadata.answer==() {
            this=finish(this,["UTF-8 run omitted output facts"]); return;
        }
        let correct=true;
        for sample in result.series.answer { if sample!=3.0 { correct=false; } }
        this.failures.push(t_true(result.series.answer.len()==3 && correct
            && result.metadata.answer.unit=="K","original UTF-8 source compiles and returns its exact trajectory"));
        let invalid=editor::new_scratch("model Invalid ﻿ Real x; end Invalid;","Invalid encoding mark");
        if invalid.ok!=true { this=finish(this,["negative source admission failed"]); return; }
        this.bad=invalid.doc_id; this.docs.push(this.bad); this.phase="negative"; return;
    }
    if this.phase=="negative" {
        let diagnostics=query("GetDiagnostics",#{doc_id:this.bad});
        if diagnostics==() || diagnostics.available!=true { return; }
        let parser=();
        for channel in diagnostics.channels { if channel.id=="modelica.parser" { parser=channel; } }
        if parser==() || parser.state!="ready" { return; }
        let source=query("GetDocumentSource",#{doc_id:this.bad});
        if source==() || parser.generation!=source.generation { return; }
        let status=query("CompileStatus",#{doc_id:this.bad});
        let owned_error=false;
        for finding in diagnostics.diagnostics {
            if finding.source=="rumoca-parser" && finding.code=="syntax"
                && finding.severity=="error" { owned_error=true; }
        }
        this.failures.push(t_true(status.ast_parsed!=true && owned_error
            && parser.generation==source.generation,
            "interior BOM has a current parser diagnostic without a crash"));
        this=finish(this,this.failures);
    }
}
