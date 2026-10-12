// Source queries preserve qualified declaration identity and reject ambiguity.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "DescribeModel"], required_inputs: []}
}
fn finish(state, failures) {
    state.done=true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures,"MODELICA CLASS IDENTITY","MODELICA_CLASS_IDENTITY"); state
}
fn input_unit(facts) {
    if facts == () || facts.inputs == () { return (); }
    for item in facts.inputs { if item.name == "marker" { return item.unit; } }
    ()
}
fn has_member(facts, name) {
    if facts == () || facts.inherited_members == () { return false; }
    for item in facts.inherited_members { if item.name == name { return true; } }
    false
}
fn on_start(me, ctx) {
    this=#{done:false,ticks:0,doc:()};
    let opened=editor::new_scratch(`within SourceSpace;
package QueryIdentity
  model FirstBase parameter Real first=1; end FirstBase;
  model SecondBase parameter Real second=2; end SecondBase;
  package A
    model Shared extends QueryIdentity.FirstBase; input Real marker(unit="K")=1; end Shared;
  end A;
  package B
    model Shared extends QueryIdentity.SecondBase; input Real marker(unit="kg")=2; end Shared;
  end B;
  model 'Quoted.Name' input Real marker(unit="m")=3; end 'Quoted.Name';
  connector ScalarInput = input Real;
  model Broken extends Missing; Real local; end Broken;
end QueryIdentity;`,"Class identity facts");
    if opened.ok!=true { this=finish(this,["scratch source admission failed"]); return; }
    this.doc=opened.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks+=1;
    if this.ticks>3000 { this=finish(this,["class identity admission timed out"]); return; }
    let status=query("CompileStatus",#{doc_id:this.doc});
    if status == () || status.ast_parsed!=true { return; }
    let first=query("DescribeModel",#{doc_id:this.doc,class:"SourceSpace.QueryIdentity.A.Shared"});
    let second=query("DescribeModel",#{doc_id:this.doc,class:"SourceSpace.QueryIdentity.B.Shared"});
    if first.ok==false || second.ok==false { return; }
    let quoted=query("DescribeModel",#{doc_id:this.doc,class:"SourceSpace.QueryIdentity.'Quoted.Name'"});
    let relative=query("DescribeModel",#{doc_id:this.doc,class:"QueryIdentity.B.Shared"});
    let broken=query("DescribeModel",#{doc_id:this.doc,class:"SourceSpace.QueryIdentity.Broken"});
    let primitive=query("DescribeModel",#{doc_id:this.doc,class:"SourceSpace.QueryIdentity.ScalarInput"});
    let bare_quoted=query("DescribeModel",#{doc_id:this.doc,class:"'Quoted.Name'"});
    let ambiguous=query("DescribeModel",#{doc_id:this.doc,class:"Shared"});
    let empty=query("DescribeModel",#{doc_id:this.doc,class:""});
    let invalid=query("DescribeModel",#{doc_id:this.doc,class:"Other.QueryIdentity.A.Shared"});
    this=finish(this,[
        t_true(first.class_name=="SourceSpace.QueryIdentity.A.Shared","response retains the authored within package"),
        t_true(input_unit(relative)=="kg" && relative.class_name=="SourceSpace.QueryIdentity.B.Shared","document-relative request resolves to its authored identity"),
        t_true(primitive.class_name=="SourceSpace.QueryIdentity.ScalarInput" && primitive.inherited_members.len()==0,"primitive-derived connector has a complete empty member set"),
        t_true(bare_quoted.class_name=="SourceSpace.QueryIdentity.'Quoted.Name'" && input_unit(bare_quoted)=="m","bare quoted identifier admits its unique declaration"),
        t_true(broken.ok==false,"unresolved inheritance has a terminal diagnostic"),
        t_true(input_unit(first)=="K","first qualified class retains its own declarations"),
        t_true(input_unit(second)=="kg","second qualified class retains its own declarations"),
        t_true(has_member(first,"first") && !has_member(first,"second"),"first class inherits its qualified base"),
        t_true(has_member(second,"second") && !has_member(second,"first"),"second class inherits its qualified base"),
        t_true(input_unit(quoted)=="m","quoted dotted identifier retains declaration identity"),
        t_true(ambiguous.ok==false,"ambiguous bare class query is terminal"),
        t_true(empty.ok==false,"explicit empty class cannot choose a default declaration"),
        t_true(invalid.ok==false,"unrelated package prefix cannot select a local declaration")
    ]);
}
