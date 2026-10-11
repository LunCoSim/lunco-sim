// Public authoring facts use Rumoca's expression and decoded-string contracts.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "DescribeModel"], required_inputs: []}
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA METADATA PROJECTION", "MODELICA_METADATA_PROJECTION");
    state
}
fn on_start(me, ctx) {
    this = #{done: false, ticks: 0, doc: ()};
    let created = editor::new_scratch(`model Metadata
  parameter Real gain = 2;
  input Real drive(unit="\"K\"\\n") "\"Quoted\" " + "description";
  input Real computed = sin(gain);
  Real value(min=if gain > 0 then sin(gain) else -1);
end Metadata;`, "Metadata contract");
    if created.ok != true { this = finish(this, ["source admission failed"]); return; }
    this.doc = created.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 3000 { this = finish(this, ["metadata admission timed out"]); return; }
    let status = query("CompileStatus", #{doc_id: this.doc});
    if status.ast_parsed != true { return; }
    let facts = query("DescribeModel", #{doc_id: this.doc, class: "Metadata"});
    if facts.components == () || facts.inputs == () { return; }
    let value = (); let drive = (); let input = (); let computed = ();
    for item in facts.components {
        if item.name == "value" { value = item; }
        if item.name == "drive" { drive = item; }
    }
    for item in facts.inputs {
        if item.name == "drive" { input = item; }
        if item.name == "computed" { computed = item; }
    }
    if value == () || drive == () || input == () || computed == () {
        this = finish(this, ["authored declarations are missing"]); return;
    }
    this = finish(this, [
        t_true(value.modifications.min == "if gain > 0 then sin(gain) else -1",
            "complex modifier uses Rumoca's complete expression rendering"),
        t_true(drive.description == "\"Quoted\" description",
            "description preserves literal quotes and authored concatenation"),
        t_true(input.unit == "\"K\"\\n",
            "decoded unit preserves literal quotes and backslash characters"),
        t_true(input["default"] == 0.0, "Real input metadata retains Rumoca's semantic start default"),
        t_true(computed["default"] == (), "non-literal binding cannot become a start value")
    ]);
}
