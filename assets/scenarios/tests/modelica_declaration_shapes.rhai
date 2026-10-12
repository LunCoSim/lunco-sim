// DescribeModel preserves declaration dimensions independently of type spelling.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "DescribeModel"], required_inputs: []}
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA DECLARATION SHAPES", "MODELICA_DECLARATION_SHAPES");
    state
}
fn on_start(me, params) {
    this = #{done: false, doc: (), ticks: 0};
    let opened = editor::new_scratch(`model ShapeFacts
      type 'Bracket[Scalar]' = Real;
      parameter Integer n=2;
      parameter Real literal[2]={1,2};
      parameter Real symbolic[n]=fill(1,n);
      parameter Real[2,3] matrix=ones(2,3);
      parameter 'Bracket[Scalar]' scalar=1;
      parameter String text="label";
    end ShapeFacts;`, "Declaration dimensions");
    if opened.ok != true { this = finish(this, [opened.to_string()]); return; }
    this.doc = opened.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3000 { this = finish(this, ["declaration shape facts timed out"]); return; }
    let parsed = query("CompileStatus", #{doc_id: this.doc});
    if parsed == () || parsed.ast_parsed != true { return; }
    let facts = query("DescribeModel", #{doc_id: this.doc, class: "ShapeFacts"});
    if facts == () || facts.ok == false || facts.parameters == () { return; }
    let checks = [];
    for parameter in facts.parameters {
        let expected = parameter.name == "literal" || parameter.name == "symbolic" || parameter.name == "matrix";
        checks.push(t_true(parameter.has_array_dimensions == expected,
            parameter.name + " retains its native declaration dimension fact"));
    }
    checks.push(t_true(facts.parameters.len() == 6,
        "shape facts cover literal, symbolic, type-prefix dimensions and scalar declarations"));
    this = finish(this, checks);
}
