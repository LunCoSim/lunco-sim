// @timing presentation
// Retained channel membership follows the admitted solver session, not draft edits.
const TEST_KIND = "editor";
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["CompileStatus", "GetDocumentSource", "DescribeModel",
        "SnapshotVariables", "ListTelemetryChannels", "QueryTelemetryHistory", "GetReadiness"],
      required_inputs: []}
}
fn source(replacement) {
    if replacement {
        return `model TelemetryReplacementProbe
  Real x(unit="kg", start=8, fixed=true) "replacement x";
equation
  der(x)=0;
end TelemetryReplacementProbe;`;
    }
    `model TelemetryReplacementProbe
  Real x(unit="m", start=4, fixed=true) "initial x";
  Real y(unit="s", start=5, fixed=true) "archived y";
equation
  der(x)=0;
  der(y)=0;
end TelemetryReplacementProbe;`
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () {
        return command_result(value.id);
    }
    value
}
fn channel(name) {
    let matches = [];
    for item in query("ListTelemetryChannels", #{name: name}).channels {
        if item.model_class == "TelemetryReplacementProbe" && item.model_variable == name {
            matches.push(item);
        }
    }
    if matches.len() != 1 { return (); }
    matches[0]
}
fn cleanup(state, checks) {
    state.checks += checks;
    state.phase = "cleanup";
    state.cleanup_ticks = 0;
    state.pending = editor::close_scratch(state.doc);
    state.doc = ();
    state
}
fn on_start(me, ctx) {
    this = #{done: false, ticks: 0, phase: "parse", checks: [], doc: (), pending: ()};
    let created = editor::new_scratch(source(false), "Telemetry session replacement");
    if created.ok != true {
        report_verdict(["scratch creation failed: " + created.to_string()],
            "MODELICA TELEMETRY REPLACEMENT", "MODELICA_TELEMETRY_REPLACEMENT");
        this.done = true;
        return;
    }
    this.doc = created.doc_id;
}
fn on_tick(me, ctx) {
    if this.done { return; }
    if this.phase == "cleanup" {
        this.cleanup_ticks += 1;
        let result = ack(this.pending);
        if result != () && result.status == "pending" && this.cleanup_ticks < 300 { return; }
        let retired = channel("x") == () && channel("y") == ();
        if !retired && this.cleanup_ticks < 300 { return; }
        this.checks.push(t_true(result != () && result.ok == true,
            "scratch close completes successfully"));
        this.checks.push(t_true(retired,
            "document closure removes both current and archived channel descriptors"));
        report_verdict(this.checks, "MODELICA TELEMETRY REPLACEMENT", "MODELICA_TELEMETRY_REPLACEMENT");
        this.done = true;
        return;
    }
    this.ticks += 1;
    if this.ticks > 3600 {
        this = cleanup(this, ["telemetry replacement timed out in " + this.phase]);
        return;
    }
    let status = query("CompileStatus", #{doc_id: this.doc});
    if status.state == "error" && this.phase != "reject" {
        this = cleanup(this, ["compile failed: " + status.to_string()]);
        return;
    }
    if this.phase == "parse" {
        if status.ast_parsed != true { return; }
        this.pending = cmd("CompileModel", #{doc_id: this.doc,
            class: "MissingTelemetryClass", resume_after_compile: false});
        this.phase = "reject";
        return;
    }
    let result = ack(this.pending);
    if result == () || result.status == "pending" { return; }
    if this.phase == "reject" {
        this.checks.push(t_true(result.ok == false && result.error != ()
            && !query("GetReadiness").faulted,
            "an invalid class has a terminal command rejection without faulting the world"));
        this.pending = cmd("CompileModel", #{doc_id: this.doc,
            class: "TelemetryReplacementProbe", resume_after_compile: false, force: true});
        this.phase = "initial";
        return;
    }
    if result.ok != true {
        this = cleanup(this, ["command failed in " + this.phase + ": " + result.to_string()]);
        return;
    }
    if this.phase == "draft" {
        let current = query("GetDocumentSource", #{doc_id: this.doc});
        if current.generation == this.generation || status.ast_parsed != true { return; }
        let declarations = query("DescribeModel", #{doc_id: this.doc, class: "TelemetryReplacementProbe"});
        let replacement_parsed = false;
        let has_y = false;
        if declarations.components != () {
            for item in declarations.components {
                if item.name == "x" { replacement_parsed = item.modifications.unit == "\"kg\""; }
                if item.name == "y" { has_y = true; }
            }
        }
        if !replacement_parsed || has_y { return; }
        let x = channel("x"); let y = channel("y");
        this.checks.push(t_true(x != () && y != () && x.active && y.active
            && x.unit == "m" && y.samples == this.y_samples.len(),
            "removing a draft declaration does not retire the admitted live channel"));
        this.pending = cmd("CompileModel", #{doc_id: this.doc,
            class: "TelemetryReplacementProbe", resume_after_compile: false});
        this.phase = "replacement";
        return;
    }
    let snapshot = query("SnapshotVariables", #{doc_id: this.doc});
    if snapshot.compiled != true || status.is_compiling == true { return; }
    let x = channel("x"); let y = channel("y");
    if this.phase == "initial" {
        if snapshot.variables.x != 4.0 || snapshot.variables.y != 5.0
            || x == () || y == () || x.samples == 0 || y.samples == 0 { return; }
        this.checks.push(t_true(x.active && y.active && x.unit == "m"
            && x.description == "initial x" && y.unit == "s" && y.description == "archived y",
            "initial session publishes both active channels with checked descriptors"));
        this.x_key = x.key; this.y_key = y.key;
        this.y_samples = query("QueryTelemetryHistory", #{key: y.key, limit: 10}).samples;
        this.checks.push(t_true(this.y_samples.len() == 1 && this.y_samples[0].t == 0.0
            && this.y_samples[0].v == 5.0, "paused initial publisher retains its t0 sample"));
        let current = query("GetDocumentSource", #{doc_id: this.doc});
        this.generation = current.generation;
        this.pending = editor::replace_source(this.doc, source(true), current.generation);
        this.phase = "draft";
        return;
    }
    if this.phase == "replacement" {
        if snapshot.variables.x != 8.0 || x == () || x.description != "replacement x" || x.samples == 0 { return; }
        let current_history = query("QueryTelemetryHistory", #{key: x.key, limit: 10}).samples;
        this.checks.push(t_true(snapshot.variables.y == () && x.active && x.key == this.x_key
            && x.unit == "kg" && x.model_variable == "x" && x.provenance == "modelica",
            "replacement reuses the current channel identity with updated checked descriptors"));
        this.checks.push(t_true(current_history.len() == 1 && current_history[0].t == 0.0
            && current_history[0].v == 8.0,
            "replacement starts a fresh current-channel history without previous-session samples"));
        this.checks.push(t_true(y != () && !y.active && y.key == this.y_key && y.unit == "s"
            && y.description == "archived y" && y.samples == this.y_samples.len(),
            "removed variable remains archived with its prior descriptor and retained sample count"));
        if y != () {
            let archived = query("QueryTelemetryHistory", #{key: y.key, limit: 10}).samples;
            this.checks.push(t_true(archived == this.y_samples,
                "removed variable history remains unchanged and queryable after recompilation"));
        }
        this = cleanup(this, []);
    }
}
