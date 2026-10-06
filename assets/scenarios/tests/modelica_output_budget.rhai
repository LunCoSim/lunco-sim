// Production output-storage admission regression. Attach to an existing scene
// root in an owned Modelica host with experiments.max_parallel=1 and the
// documented default 8,000,000 scalar-value budget.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult"], required_inputs: [] }
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn ready(doc, class) {
    let status = query("CompileStatus", #{doc_id: doc});
    status != () && status.ok != false && status.ast_parsed == true
        && status.candidates != () && status.candidates.contains(class)
}
fn finish(state, failures) {
    if state.done { return state; }
    state.done = true;
    for doc in state.docs { editor::close_scratch(doc); }
    report_verdict(failures, "MODELICA OUTPUT BUDGET", "MODELICA_OUTPUT_BUDGET");
    state
}
fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.phase = "prepare"; this.docs = [];
    let bad = editor::new_scratch(
        "model OutputBudgetProbe Real z[64](each start=1, each fixed=true); equation for i in 1:64 loop der(z[i])=-z[i]; end for; end OutputBudgetProbe;",
        "Output budget negative");
    let good = editor::new_scratch(
        "model BudgetSuccessor Real x(start=5, fixed=true); equation der(x)=0; end BudgetSuccessor;",
        "Output budget valid successor");
    if bad != () && bad.ok == true { this.docs.push(bad.doc_id); }
    if good != () && good.ok == true { this.docs.push(good.doc_id); }
    if bad == () || good == () || bad.ok != true || good.ok != true {
        this = finish(this, ["scratch creation failed: " + [bad, good].to_string()]); return;
    }
    this.bad = bad.doc_id; this.good = good.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["output budget timed out at " + this.phase]); return; }
    if this.phase == "prepare" {
        if !ready(this.bad, "OutputBudgetProbe") || !ready(this.good, "BudgetSuccessor") { return; }
        this.bad_ack = cmd("RunExperiment", #{doc_id: this.bad, class: "OutputBudgetProbe", t_end: 1.0, n_intervals: 200000});
        this.good_ack = cmd("RunExperiment", #{doc_id: this.good, class: "BudgetSuccessor", t_end: 0.1, n_intervals: 10});
        this.phase = "admitted"; return;
    }
    if this.phase == "admitted" {
        let bad = ack(this.bad_ack); let good = ack(this.good_ack);
        if bad == () || good == () || bad.status == "pending" || good.status == "pending" { return; }
        if bad.ok != true || good.ok != true || bad.data == () || good.data == ()
            || bad.data.experiment_id == () || good.data.experiment_id == () {
            this = finish(this, ["valid bounds were not admitted: " + [bad, good].to_string()]); return;
        }
        this.bad_run = bad.data.experiment_id; this.good_run = good.data.experiment_id;
        this.phase = "terminal";
    }
    if this.phase == "terminal" {
        let bad = query("RunStatus", #{experiment_id: this.bad_run});
        let good = query("RunStatus", #{experiment_id: this.good_run});
        if bad == () || good == () || bad.ok == false || good.ok == false { return; }
        if bad.state == "queued" || bad.state == "running" || bad.state == "pending"
            || good.state == "queued" || good.state == "running" || good.state == "pending" { return; }
        let result = query("GetExperimentResult", #{experiment_id: this.good_run, variables: ["x"]});
        let valid = result != () && result.ok != false && result.series != ()
            && result.series["x"] != () && result.series["x"].len() > 0;
        if valid { for x in result.series["x"] { valid = valid && x.is_finite() && (x - 5.0).abs() < 1.0e-10; } }
        this = finish(this, [
            t_true(bad.state == "failed" && bad.error != () && bad.error.contains("output budget"),
                "actual lowered output dimensions fail visibly before trajectory allocation: " + bad.to_string()),
            t_true(bad.has_result == false && bad.partial == false, "over-budget admission publishes no trajectory"),
            t_true(good.state == "done" && valid, "valid successor completes after failed run releases its only scheduler slot")
        ]);
    }
}
