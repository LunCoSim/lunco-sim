// Scoped type declarations own authoring input/output roles and alias dimensions.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "DescribeModel"], required_inputs: []}
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA TYPE PREFIXES", "MODELICA_TYPE_PREFIXES");
    state
}
fn names(items) { let result=[]; for item in items { result.push(item.name); } result.sort(); result }
fn on_start(me, params) {
    this = #{done: false, doc: (), ticks: 0};
    let opened = editor::new_scratch(`package PrefixFacts
      connector Receiver = input Real;
      connector Sender = output Real;
      connector ReceiverAgain = Receiver;
      type Vector = Real[2];
      type Caption = String;
      record FauxInput Real pressure; end FauxInput;
      record FauxOutput Real pressure; end FauxOutput;
      model Base
        Receiver u; Sender y;
      end Base;
      model Probe
        extends Base;
        ReceiverAgain u2;
        FauxInput internal_a; FauxOutput internal_b;
        parameter Vector values={1,2};
        parameter Caption label="caption";
      equation y=u+u2; internal_a.pressure=1; internal_b.pressure=2;
      end Probe;
      connector CycleA = CycleB;
      connector CycleB = CycleA;
      model Cyclic CycleA x; end Cyclic;
      model Missing Absent x; end Missing;
    end PrefixFacts;`, "Scoped type prefixes");
    if opened.ok != true { this = finish(this, [opened.to_string()]); return; }
    this.doc = opened.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3000 { this = finish(this, ["type prefix facts timed out"]); return; }
    let parsed = query("CompileStatus", #{doc_id: this.doc});
    if parsed == () || parsed.ast_parsed != true { return; }
    let facts = query("DescribeModel", #{doc_id: this.doc, class: "PrefixFacts.Probe"});
    if facts == () || facts.ok == false { return; }
    let vector=(); let caption=();
    for parameter in facts.parameters {
        if parameter.name=="values" { vector=parameter; }
        if parameter.name=="label" { caption=parameter; }
    }
    let cyclic=query("DescribeModel", #{doc_id: this.doc, class: "PrefixFacts.Cyclic"});
    let missing=query("DescribeModel", #{doc_id: this.doc, class: "PrefixFacts.Missing"});
    this = finish(this, [
        t_true(names(facts.inputs)==["u","u2"], "input roles follow scoped alias prefixes without name suffixes"),
        t_true(names(facts.outputs)==["y"], "output roles follow type prefixes; record names do not create outputs"),
        t_true(vector!=() && vector.has_array_dimensions==true && vector.scalar_type=="Real", "array dimensions and primitive type survive a named type alias"),
        t_true(caption!=() && caption.scalar_type=="String", "String aliases retain their primitive type"),
        t_true(cyclic!=() && cyclic.ok==false, "cyclic type prefixes produce a terminal query diagnostic"),
        t_true(missing!=() && missing.ok==false, "missing types produce a terminal query diagnostic")
    ]);
}
