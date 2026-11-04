// Parameter source ownership and terminal checked session replacement.
// @timing presentation
const TEST_KIND = "editor";
import "lunco://scripting/tools/modelica_editor" as editor;
fn simulation_dependencies(me, ctx) {
    #{modelica_entities:[], entity_reads:[], entity_writes:[],
      query_reads:["CompileStatus","SnapshotVariables","GetDocumentSource","ListOpenDocuments"], required_inputs:[]}
}
fn terminal(value) {
    if value!=() && value.status=="pending" && value.id!=() {return command_result(value.id);}
    value
}
fn finish(state, checks) {
    state.done=true;
    for doc in state.docs {editor::close_scratch(doc);}
    report_verdict(checks,"MODELICA PARAMETERS","MODELICA_PARAMETERS");state
}
fn on_start(me, ctx) {
    this=#{done:false,elapsed:0.0,phase:"parse",docs:[],checks:[]};
    let helper=editor::new_scratch(`model ParameterContractHelper
        output Real value; equation value=4; end ParameterContractHelper;`,"Parameter helper");
    if helper.ok!=true {this=finish(this,["parameter helper source was not created"]);return;}
    this.docs.push(helper.doc_id);
    let main=editor::new_scratch(`package ParameterContract
      model First parameter Real gain=1; output Real value; equation value=gain; end First;
      model Second
        parameter Real gain(min=0,max=10)=2;
        parameter Real keep=3;
        final parameter Real fixedGain=4;
        ParameterContractHelper helper;
        output Real value;
      equation value=(gain+helper.value)*keep; end Second;
    end ParameterContract;`,"Parameter selected class");
    if main.ok!=true {this=finish(this,["parameter source was not created"]);return;}
    this.doc=main.doc_id;this.docs.push(main.doc_id);
    if ctx.no_document_entity==() || ctx.no_document_entity<0 {
        this=finish(this,["the no-document USD participant fixture is required"]);return;
    }
    let target=ctx.no_document_entity;
    let before=[get(target,"ModelicaModel.session_id"),get(target,"ModelicaModel.is_stepping"),
        get(target,"ModelicaModel.is_compiling"),get(target,"ModelicaModel.parameters")];
    this.pending=cmd("SetModelicaParameter",#{entity:target,key:"gain",value:9.0});
    let after=[get(target,"ModelicaModel.session_id"),get(target,"ModelicaModel.is_stepping"),
        get(target,"ModelicaModel.is_compiling"),get(target,"ModelicaModel.parameters")];
    this.checks.push(t_true(before==after,
        "no-document rejection leaves the participant's session, transaction and parameters intact"));
    this.phase="no-document";
}
fn on_tick(me, ctx) {
    if this.done {return;}
    this.elapsed+=execution_context().delta_seconds;
    if this.elapsed>60.0 {this=finish(this,["parameter contract timed out at "+this.phase]);return;}
    if this.phase=="no-document" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        this.checks.push(t_true(result.ok==false && result.error.contains("no editable Modelica document"),
            "a USD participant without a document returns its terminal ownership rejection"));
        this.phase="parse";
    }
    if this.phase=="parse" {
        for doc in this.docs {if query("CompileStatus",#{doc_id:doc}).ast_parsed!=true {return;}}
        this.pending=cmd("CompileModel",#{doc_id:this.doc,class:"ParameterContract.Second",resume_after_compile:false});
        this.phase="compile";return;
    }
    if this.phase=="compile" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["selected parameter class did not compile: "+result]);return;}
        this.entity=find("ParameterContract.Second");
        if this.entity<0 {return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(sample.compiled && sample.variables.value==18.0,
            "selected second class uses its admitted sibling source and parameters"));
        this.before=query("GetDocumentSource",#{doc_id:this.doc});
        let count=query("ListOpenDocuments").count;
        let action=cmd("OpenClass",#{qualified:"ParameterContract.Second",
            action:#{kind:"Duplicate",name:"RejectedParameterCopy",unexpected:true}});
        this.checks.push(t_true(action.ok==false && query("ListOpenDocuments").count==count,
            "unknown tagged action fields reject before opening a copy"));
        let graphic=cmd("ApplyModelicaOps",#{doc_id:this.doc,ops:[#{AddIconGraphic:#{
            class:"ParameterContract.Second",graphic:#{kind:"Text",x1:-1.0,y1:-1.0,
                x2:1.0,y2:1.0,text:"probe",font_szie:12.0}}}]});
        this.checks.push(t_true(graphic.ok==false
            && query("GetDocumentSource",#{doc_id:this.doc}).generation==this.before.generation,
            "unknown tagged graphic fields reject before editing source"));
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"missing",value:5.0});
        this.phase="unknown";return;
    }
    if this.phase=="unknown" || this.phase=="bounds" || this.phase=="final" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let after=query("GetDocumentSource",#{doc_id:this.doc});
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(result.ok==false && after.generation==this.before.generation
            && sample.compiled && sample.parameters.gain==2.0 && sample.variables.value==18.0,
            this.phase+" parameter rejection preserves the source and installed session"));
        if this.phase=="unknown" {
            this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:11.0});this.phase="bounds";return;
        }
        if this.phase=="bounds" {
            this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"fixedGain",value:8.0});this.phase="final";return;
        }
        let resumed=cmd("ResumeActiveModel",#{doc_id:this.doc});
        if resumed.ok!=true {this=finish(this,["parameter participant did not resume: "+resumed]);return;}
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:6.0});
        let busy=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:9.0});
        this.checks.push(t_true(busy.ok==false && busy.error.contains("outstanding transaction"),
            "another edit cannot supersede the admitted parameter transaction"));
        this.phase="replace";return;
    }
    if this.phase=="replace" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        if result.ok!=true {this=finish(this,["parameter replacement failed: "+result]);return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        let source=query("GetDocumentSource",#{doc_id:this.doc});
        this.checks.push(t_true(sample.compiled && sample.parameters.gain==6.0 && sample.parameters.keep==3.0
            && sample.variables.value==30.0,"terminal success installs the edited value with the retained sibling closure"));
        this.checks.push(t_true(source.source.contains("model First parameter Real gain=1;")
            && source.generation>this.before.generation,"parameter authoring changes only the selected class"));
        cmd("PauseActiveModel",#{doc_id:this.doc});
        this.pending=editor::replace_source(this.doc,source.source+"\n",source.generation);
        this.stale_generation=query("GetDocumentSource",#{doc_id:this.doc}).generation;
        this.pending=cmd("SetModelicaParameter",#{entity:this.entity,key:"gain",value:7.0});
        this.phase="stale";return;
    }
    if this.phase=="stale" {
        let result=terminal(this.pending);if result.status=="pending" {return;}
        let sample=query("SnapshotVariables",#{doc_id:this.doc});
        this.checks.push(t_true(result.ok==false && sample.compiled && sample.parameters.gain==6.0
            && query("GetDocumentSource",#{doc_id:this.doc}).generation==this.stale_generation,
            "an uncompiled source edit cannot start a parameter transaction"));
        this=finish(this,this.checks);
    }
}
