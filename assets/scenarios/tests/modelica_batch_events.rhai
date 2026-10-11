// Checked batch trajectories retain event samples independently of output spacing.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "RunStatus", "GetExperimentResult"], required_inputs: []}
}
fn finish(state, failures) {
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA BATCH EVENTS", "MODELICA_BATCH_EVENTS");
    state
}
fn on_start(me, params) {
    this = #{done: false, ticks: 0, doc: (), phase: "prepare"};
    let created = editor::new_scratch(`model BatchEventProbe
  Real x(start=0, fixed=true);
equation
  der(x)=0;
  when sample(0.001, 0.002) then
    reinit(x, pre(x)+1);
  end when;
end BatchEventProbe;`, "Batch event trajectory");
    if created.ok != true { this = finish(this, ["source admission failed"]); return; }
    this.doc = created.doc_id;
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["batch event timeout in " + this.phase]); return; }
    if this.phase == "prepare" {
        if query("CompileStatus", #{doc_id: this.doc}).ast_parsed != true { return; }
        this.ack = cmd("RunExperiment", #{doc_id: this.doc, class: "BatchEventProbe",
          t_end: 0.1, n_intervals: 10, solver: "rk45"});
        this.phase = "admitted";
        return;
    }
    if this.phase == "admitted" {
        let ack = this.ack;
        if ack.status == "pending" { ack = command_result(ack.id); }
        if ack == () || ack.status == "pending" { return; }
        if ack.ok != true || ack.data == () || ack.data.experiment_id == () {
            this = finish(this, ["run admission failed: " + ack.to_string()]); return;
        }
        this.run = ack.data.experiment_id;
        this.phase = "terminal";
    }
    let status = query("RunStatus", #{experiment_id: this.run});
    if status.state == "pending" || status.state == "queued" || status.state == "running" { return; }
    if status.state != "done" { this = finish(this, ["solve failed: " + status.to_string()]); return; }
    let result = query("GetExperimentResult", #{experiment_id: this.run, variables: ["x"]});
    if result.times == () || result.series == () || result.series.x == () || result.times.len() < 2 {
        this = finish(this, ["completed trajectory missing"]); return;
    }
    let times = result.times; let values = result.series.x;
    let complete_events = true;
    for event in 0..50 {
        let before = false; let after = false;
        let event_time = 0.001 + 0.002 * event;
        for i in 0..times.len() {
            if (times[i]-event_time).abs() < 1.0e-8 {
                before = before || (values[i]-event).abs() < 1.0e-8;
                after = after || (values[i]-event-1.0).abs() < 1.0e-8;
            }
        }
        complete_events = complete_events && before && after;
    }
    this = finish(this, [
      t_true(times.len() >= 102, "trajectory retains fifty event pairs and both endpoints: " + times.len()),
      t_true(complete_events, "all off-grid events retain their distinct pre/post states"),
      t_true((times[0]).abs() < 1.0e-12 && (times[times.len()-1]-0.1).abs() < 1.0e-8
        && (values[values.len()-1]-50.0).abs() < 1.0e-8,
        "trajectory retains its admitted horizon and all fifty reinitializations")
    ]);
}
