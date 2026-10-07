// Production regression for source-defined aggregate and qualified outputs.
// Attach to an owned Modelica-capable production host with RunScenarioAsset.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["CompileStatus", "SnapshotVariables", "RunStatus", "GetExperimentResult"], required_inputs: [] }
}
fn finish(state, failures) {
    if state.done { return state; }
    state.done = true;
    for doc in state.docs { editor::close_scratch(doc); }
    report_verdict(failures, "MODELICA AGGREGATE OUTPUT", "MODELICA_AGGREGATE_OUTPUT");
    state
}
fn prepared(doc, class) {
    let s = query("CompileStatus", #{doc_id: doc});
    s != () && s.ast_parsed == true && s.candidates != () && s.candidates.contains(class)
}
fn defaults(snapshot, expected_a) {
    let failures = [];
    for pair in [["enabled", 9.0], ["a.enabled", expected_a], ["again.enabled", 1.0],
                 ["b.enabled", 0.0], ["d.enabled", 5.0], ["expression.value", 6.0]] {
        let actual = snapshot.inputs[pair[0]];
        failures.push(t_true(actual != () && (actual - pair[1]).abs() < 1.0e-10,
            pair[0] + " retains its declaration default: " + actual.to_string()));
    }
    failures.push(t_true(snapshot.inputs["gains[1]"] == 3.0 && snapshot.inputs["gains[2]"] == 4.0,
        "array defaults retain both initialized elements"));
    let expected_y=29.0+expected_a;
    let actual_y=snapshot.variables.y;
    failures.push(t_true(actual_y!=() && actual_y.is_finite() && (actual_y-expected_y).abs()<1.0e-10,
        "aggregate output equals the continuous derivative: expected="+expected_y+", actual="+actual_y));
    for pair in [["a.y",expected_a],["again.y",1.0],["b.y",0.0],["d.y",5.0],
        ["expression.y",6.0],["local.y",1.0]] {
        let actual=snapshot.variables[pair[0]];
        failures.push(t_true(actual!=() && actual.is_finite() && (actual-pair[1]).abs()<1.0e-10,
            "qualified output reconstructs its source equation: "+pair[0]+"="+actual));
    }
    print("MODELICA_AGGREGATE_LIVE "+#{time:snapshot.t,y:actual_y,expected_y:expected_y,
        x:snapshot.variables.x,a_y:snapshot.variables["a.y"]});
    failures.push(t_true(snapshot.variables.x != () && snapshot.t > 0.0
        && (snapshot.variables.x - (29.0 + expected_a) * snapshot.t).abs() < 1.0e-8,
        "continuous state consumes class, array, internal, and expression defaults: " + snapshot.to_string()));
    failures
}
fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.phase = "prepare"; this.docs = []; this.failures = [];
    let good = editor::new_scratch(`model ScopedProbe
parameter Real observationProbeRevision=1.0;
function scale input Real x=2.0; output Real y; algorithm y:=x*3.0; end scale;
model On input Real enabled=1.0; output Real y; equation y=enabled; end On;
model Off input Real enabled=0.0; output Real y; equation y=enabled; end Off;
model Base input Real enabled=5.0; output Real y; equation y=enabled; end Base;
model Derived extends Base; end Derived;
model Expression parameter Real offset=3.0; input Real value=offset*2.0+scale()-6.0; output Real y; equation y=value; end Expression;
input Real enabled=9.0; input On a; input On again;
input Off b; input Derived d; input Expression expression;
input Real gains[2]={3.0,4.0}; On local;
output Real y; Real x(start=0.0, fixed=true);
equation y=enabled+a.y+again.y+b.y+d.y+expression.y+sum(gains)+local.y; der(x)=y;
end ScopedProbe;`, "Aggregate output positive");
    let bad = editor::new_scratch("model InvalidDefault input Real enabled=1.0; output Real y; equation y=enabled+missing; end InvalidDefault;", "Aggregate output invalid expression");
    if good.ok != true || bad.ok != true { this = finish(this, ["scratch admission failed"]); return; }
    this.good = good.doc_id; this.bad = bad.doc_id; this.docs = [this.good, this.bad];
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 12000 { this = finish(this, ["aggregate-output timed out at " + this.phase]); return; }
    if this.phase == "prepare" {
        if !prepared(this.good, "ScopedProbe") || !prepared(this.bad, "InvalidDefault") { return; }
        cmd("CompileModel", #{doc_id: this.good, class: "ScopedProbe", resume_after_compile: true});
        this.phase = "initial"; return;
    }
    if this.phase == "initial" {
        let status = query("CompileStatus", #{doc_id: this.good});
        if status.state == "error" { this = finish(this, ["positive compile failed: " + status.to_string()]); return; }
        let s = query("SnapshotVariables", #{doc_id: this.good});
        if s.compiled != true || s.t == () || s.t < 0.05 { return; }
        this.failures += defaults(s, 1.0);
        cmd("SetModelInput", #{doc_id: this.good, name: "a.enabled", value: 4.0, producer_id: 81271});
        this.before = s.t; this.before_x = s.variables.x; this.phase = "override"; return;
    }
    if this.phase == "override" {
        let s = query("SnapshotVariables", #{doc_id: this.good});
        if s.t <= this.before + 0.05 { return; }
        let live_y=s.variables.y;
        this.failures.push(t_true(live_y!=() && live_y.is_finite() && (live_y-33.0).abs()<1.0e-10,
            "aggregate output follows the qualified override: "+live_y));
        let component_y=s.variables["a.y"];
        this.failures.push(t_true(component_y!=() && component_y.is_finite() && (component_y-4.0).abs()<1.0e-10,
            "qualified component output follows the override: "+component_y));
        print("MODELICA_AGGREGATE_OVERRIDE "+#{time:s.t,y:live_y,a_y:component_y,x:s.variables.x});
        this.failures.push(t_true(s.inputs["a.enabled"] == 4.0 && s.inputs["again.enabled"] == 1.0
            && s.inputs["b.enabled"] == 0.0
            && (s.variables.x - this.before_x - 33.0 * (s.t - this.before)).abs() < 1.0e-8,
            "qualified override changes only the selected instance: " + s.to_string()));
        cmd("ResetActiveModel", #{doc_id: this.good}); this.phase = "reset"; return;
    }
    if this.phase == "reset" {
        let s = query("SnapshotVariables", #{doc_id: this.good});
        if s.variables.y == () || s.t < 0.05 { return; }
        this.failures += defaults(s, 4.0);
        let live_y=s.variables.y;
        this.failures.push(t_true(live_y!=() && live_y.is_finite() && (live_y-33.0).abs()<1.0e-10,
            "aggregate output follows the qualified override: "+live_y));
        let component_y=s.variables["a.y"];
        this.failures.push(t_true(component_y!=() && component_y.is_finite() && (component_y-4.0).abs()<1.0e-10,
            "qualified component output follows the override: "+component_y));
        print("MODELICA_AGGREGATE_OVERRIDE "+#{time:s.t,y:live_y,a_y:component_y,x:s.variables.x});
        this.failures.push(t_true(s.inputs["a.enabled"] == 4.0, "reset preserves the explicit input override"));
        cmd("CompileModel", #{doc_id: this.bad, class: "InvalidDefault"});
        this.phase = "negative"; return;
    }
    if this.phase == "negative" {
        let s = query("CompileStatus", #{doc_id: this.bad});
        if s.state != "error" { return; }
        this.failures.push(t_true(s.error_message != () && s.error_message.contains("missing"),
            "invalid default fails visibly without a fabricated value: " + s.to_string()));
        this.ack = cmd("RunExperiment", #{doc_id: this.good, class: "ScopedProbe", t_end: 0.1, n_intervals: 10});
        this.phase = "batch_ack"; return;
    }
    if this.phase == "batch_ack" {
        let ack = this.ack;
        if ack != () && ack.status == "pending" && ack.id != () { ack = command_result(ack.id); }
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true || ack.data == () || ack.data.experiment_id == () {
            this = finish(this, ["batch rejected: " + ack.to_string()]); return;
        }
        this.run = ack.data.experiment_id; this.phase = "batch"; return;
    }
    if this.phase == "batch" {
        let status = query("RunStatus", #{experiment_id: this.run});
        if status.state == "pending" || status.state == "queued" || status.state == "running" { return; }
        if status.state != "done" { this = finish(this, ["batch failed: " + status.to_string()]); return; }
        let r = query("GetExperimentResult", #{experiment_id: this.run, variables: ["x","y","a.y","again.y","b.y","d.y","expression.y","local.y"]});
        let valid = r != () && r.series != () && r.series.x != () && r.series.x.len() > 1;
        if valid {
            for i in 0..r.series.x.len() {
                let x = r.series.x[i];
                valid = valid && x.is_finite() && (x - 30.0 * r.times[i]).abs() < 1.0e-8;
            }
        }
        this.failures.push(t_true(valid, "batch derivative consumes the same scoped defaults"));
        for pair in [["y",30.0],["a.y",1.0],["again.y",1.0],["b.y",0.0],["d.y",5.0],
            ["expression.y",6.0],["local.y",1.0]] {
            let series=if r==() || r.series==() {()} else {r.series[pair[0]]};
            let correct=series!=() && r.times!=() && series.len()==r.times.len() && series.len()>1;
            if correct {for value in series {correct=correct && value.is_finite() && (value-pair[1]).abs()<1.0e-10;}}
            this.failures.push(t_true(correct,"batch reconstructs the aggregate/component output: "+pair[0]+"="+series));
        }
        print("MODELICA_AGGREGATE_BATCH "+#{
            time:if r==() {()} else {r.times},
            y:if r==() || r.series==() {()} else {r.series.y},
            x:if r==() || r.series==() {()} else {r.series.x}});
        this = finish(this, this.failures);
    }
}
