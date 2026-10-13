// Live channel facts belong to the exact checked FMI scalar inventory.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "SnapshotVariables", "ListTelemetryChannels", "GetDocumentSource", "DescribeModel", "CosimStatus"], required_inputs: []}
}
fn source(unit) {
    `model MetadataProbe
  constant Real constant_rate=3.0;
  type Distance = Real(unit="m");
  model Hot
    Real sample(unit="K") "hot reading";
  equation sample=1; end Hot;
  model Cold
    Real sample(unit="` + unit + `") "cold reading";
  equation sample=2; end Cold;
  model PlainPin
    Real v;
  equation v=3; end PlainPin;
  Hot hot; Cold cold; PlainPin p;
  Distance vector[2](start={1,2}, fixed=true, min={0,1}, max={10,20}) "vector distance";
  input Real drive(min=0,max=100)=5;
  Real untyped;
equation
  der(vector)={1,2}; untyped=constant_rate;
end MetadataProbe;`
}
fn channel(name) {
    let result = query("ListTelemetryChannels", #{name: name});
    if result == () || result.channels == () { return (); }
    let channels = [];
    for item in result.channels {
        if item.model_class == "MetadataProbe" && item.model_variable == name {
            channels.push(item);
        }
    }
    if channels.len() != 1 { return (); }
    channels[0]
}
fn checks(doc, cold_unit) {
    let snapshot = query("SnapshotVariables", #{doc_id: doc});
    let metadata = snapshot.metadata;
    let hot = channel("hot.sample"); let cold = channel("cold.sample");
    let a = channel("vector[1]"); let b = channel("vector[2]");
    let pin = channel("p.v"); let plain = channel("untyped");
    [
      t_true(snapshot.variables.untyped == 3.0,
        "Authored constant values survive checked initialization"),
      t_true(hot != () && hot.unit == "K" && hot.description == "hot reading",
        "qualified hot declaration retains its compiled unit and description"),
      t_true(cold != () && cold.unit == cold_unit && cold.description == "cold reading",
        "duplicate leaf names retain distinct compiled declaration metadata"),
      t_true(a != () && b != () && a.unit == "m" && b.unit == "m"
          && a.description == "vector distance" && b.description == "vector distance",
        "scalar array entries retain inherited type units and declaration descriptions"),
      t_true(pin != () && pin.unit == () && pin.description == (),
        "a connector-shaped name cannot fabricate an electrical contract"),
      t_true(plain != () && plain.unit == () && plain.description == (),
        "an undocumented declaration remains undocumented"),
      t_true(metadata != () && metadata.drive != () && metadata.drive.minimum == 0.0 && metadata.drive.maximum == 100.0,
        "input controls consume checked declaration bounds"),
      t_true(metadata != () && metadata["vector[1]"] != () && metadata["vector[2]"] != () && metadata["vector[1]"].minimum == 0.0 && metadata["vector[1]"].maximum == 10.0
        && metadata["vector[2]"].minimum == 1.0 && metadata["vector[2]"].maximum == 20.0,
        "scalar array bounds retain their respective checked elements")
    ]
}
fn finish(state, failures) {
    if state.doc != () { editor::close_scratch(state.doc); }
    state.phase = "cleanup";
    state.cleanup_ticks = 0;
    state.failures = failures;
    state
}
fn on_start(me, ctx) {
    this = #{done: false, ticks: 0, doc: (), phase: "prepare", failures: [],
      participants: query("CosimStatus").synchronization.active_participants};
    let created = editor::new_scratch(source("V"), "Compiled metadata contract");
    if created.ok != true { this = finish(this, ["source admission failed"]); return; }
    this.doc = created.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    if this.phase == "cleanup" {
        this.cleanup_ticks += 1;
        let retired = query("CosimStatus").synchronization.active_participants == this.participants;
        if !retired && this.cleanup_ticks < 300 { return; }
        this.failures.push(t_true(retired, "scratch closure retires its live Modelica participant"));
        report_verdict(this.failures, "MODELICA COMPILED METADATA", "MODELICA_COMPILED_METADATA");
        this.done = true;
        return;
    }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["compiled metadata timed out in " + this.phase]); return; }
    let status = query("CompileStatus", #{doc_id: this.doc});
    if status.state == "error" { this = finish(this, ["compile failed: " + status.to_string()]); return; }
    if this.phase == "prepare" {
        if status.ast_parsed != true { return; }
        cmd("CompileModel", #{doc_id: this.doc, class: "MetadataProbe", resume_after_compile: true});
        this.phase = "initial"; return;
    }
    if this.phase == "draft" {
        let doc = query("GetDocumentSource", #{doc_id: this.doc});
        if doc.generation == this.generation || status.ast_parsed != true { return; }
        let draft = query("DescribeModel", #{doc_id: this.doc, class: "MetadataProbe.Cold"});
        let unit = ();
        if draft.components != () {
            for item in draft.components { if item.name == "sample" { unit = item.modifications.unit; } }
        }
        if unit != "\"kg\"" { return; }
        this.failures += checks(this.doc, "V");
        cmd("CompileModel", #{doc_id: this.doc, class: "MetadataProbe", resume_after_compile: true});
        this.phase = "recompile"; return;
    }
    let sample = query("SnapshotVariables", #{doc_id: this.doc});
    if sample.compiled != true || sample.variables["vector[2]"] == () || sample.t == () || (this.phase != "reset" && sample.t < 0.05) || channel("vector[2]") == () { return; }
    if this.phase == "initial" {
        this.failures += checks(this.doc, "V");
        for failure in this.failures { if failure != "" { this = finish(this, this.failures); return; } }
        cmd("PauseActiveModel", #{doc_id: this.doc});
        cmd("ResetActiveModel", #{doc_id: this.doc}); this.phase = "reset"; return;
    }
    if this.phase == "reset" {
        if sample.t != 0.0 || sample.variables["vector[2]"] != 2.0 { return; }
        this.failures += checks(this.doc, "V");
        let filtered = query("SnapshotVariables", #{doc_id: this.doc, names: ["vector[1]"]});
        this.failures.push(t_true(filtered.metadata != () && filtered.metadata.len() == 1 && filtered.metadata["vector[1]"] != () && filtered.metadata["vector[1]"].unit == "m"
            && filtered.variables["vector[2]"] == (), "metadata and values share the exact requested name filter"));
        this.failures.push(t_true(sample.variables["vector[1]"] == 1.0,
            "reset installs checked initial values with the new session metadata"));
        cmd("PauseActiveModel", #{doc_id: this.doc});
        let doc = query("GetDocumentSource", #{doc_id: this.doc});
        this.generation = doc.generation;
        let replaced = editor::replace_source(this.doc, source("kg"), doc.generation);
        if replaced.ok != true { this = finish(this, ["draft source edit failed"]); return; }
        this.phase = "draft"; return;
    }
    if this.phase == "recompile" {
        this.failures += checks(this.doc, "kg");
        this = finish(this, this.failures);
    }
}
