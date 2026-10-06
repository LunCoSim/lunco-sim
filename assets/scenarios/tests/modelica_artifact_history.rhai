// Run in two fresh owned Application-scoped sessions. Seed root is a fresh
// directory containing an API-authored entry scene and no Twin manifest;
// restore opens that existing Twin. Parameters: mode, root, entry_scene.
// After seed PASS, inspect ListRuns for the ArtifactHistoryProbe UUID/CID and
// verify <root>/results/<UUID>.json is durable before Exit. Place a malformed
// optional neighbor with another UUID, then restart and pass seed_id, seed_cid,
// corrupt_id to restore. Persistence JSON fixtures are external inputs; this
// gate never authors USD or treats a current source as an archived trajectory.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ReadActiveTwinContract", "CompileStatus", "RunStatus", "ListRuns", "GetExperimentResult"], required_inputs: [] }
}
fn ack(value) {
    if value != () && value.status == "pending" && value.id != () { return command_result(value.id); }
    value
}
fn ready(doc) {
    let status = query("CompileStatus", #{doc_id: doc});
    status != () && status.ok != false && status.ast_parsed == true
        && status.candidates != () && status.candidates.contains("ArtifactHistoryProbe")
}
fn result(id) { query("GetExperimentResult", #{experiment_id: id, variables: ["x"]}) }
fn values_are(record, expected) {
    if record == () || record.ok == false || record.series == () || record.series["x"] == () || record.series["x"].len() == 0 { return false; }
    let valid = true;
    for x in record.series["x"] { valid = valid && x.is_finite() && (x-expected).abs() < 1.0e-10; }
    valid
}
fn finish(state, failures) {
    if state.done { return state; }
    state.done = true;
    if state.doc != () { editor::close_scratch(state.doc); }
    report_verdict(failures, "MODELICA ARTIFACT HISTORY " + state.mode, "MODELICA_ARTIFACT_HISTORY");
    state
}
fn on_start(me, params) {
    this.done = false; this.doc = (); this.ticks = 0; this.phase = "mount";
    this.mode = params.mode; this.seed_id = params.seed_id; this.seed_cid = params.seed_cid; this.corrupt_id = params.corrupt_id;
    if (this.mode != "seed" && this.mode != "restore") || params.root == () || params.entry_scene == () {
        this = finish(this, ["mode, existing Twin root and entry_scene are required"]); return;
    }
    if this.mode == "restore" && (this.seed_id == () || this.seed_cid == () || this.corrupt_id == () || this.seed_id == this.corrupt_id) {
        this = finish(this, ["restore requires distinct seed/corrupt UUIDs and the observed seed source CID"]); return;
    }
    let twin = query("ReadActiveTwinContract", #{});
    if twin == () || twin.open != false { this = finish(this, ["gate requires fresh Application document scope"]); return; }
    this.mount_ack = if this.mode == "seed" {
        cmd("CreateTwin", #{path: params.root, name: "Artifact history owner", default_scene: params.entry_scene})
    } else {
        cmd("OpenTwin", #{path: params.root})
    };
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 6000 { this = finish(this, ["artifact gate timed out at " + this.phase]); return; }
    if this.phase == "mount" {
        let admitted = ack(this.mount_ack); if admitted == () || admitted.status == "pending" { return; }
        if admitted.ok != true { this = finish(this, ["Twin admission rejected: " + admitted.to_string()]); return; }
        let twin = query("ReadActiveTwinContract", #{});
        if twin.open != true { return; }
        this.phase = if this.mode == "restore" { "history" } else { "source" };
    }
    if this.phase == "history" {
        let archived = result(this.seed_id);
        if archived == () || archived.ok == false || archived.source_cid == () { return; }
        let corrupt = query("RunStatus", #{experiment_id: this.corrupt_id});
        if !values_are(archived,1.0) || archived.restored_history != true || archived.source_cid != this.seed_cid {
            this = finish(this, ["restored history lost its immutable source/value identity: " + archived.to_string()]); return;
        }
        if corrupt != () && corrupt.ok != false {
            this = finish(this, ["malformed optional neighbor created a runtime experiment: " + corrupt.to_string()]); return;
        }
        this.phase = "source";
    }
    if this.phase == "source" {
        let start = if this.mode == "seed" { "1" } else { "3" };
        let opened = editor::new_scratch("model ArtifactHistoryProbe Real x(start=" + start + ",fixed=true); equation der(x)=0; end ArtifactHistoryProbe;", "Artifact history current source");
        if opened == () || opened.ok != true { this = finish(this, ["source admission rejected: " + opened.to_string()]); return; }
        this.doc = opened.doc_id; this.phase = "ready"; return;
    }
    if this.phase == "ready" {
        if !ready(this.doc) { return; }
        this.run_ack = cmd("RunExperiment", #{doc_id: this.doc, class: "ArtifactHistoryProbe", t_end: 0.1, n_intervals: 10});
        this.phase = "admission"; return;
    }
    if this.phase == "admission" {
        let admitted = ack(this.run_ack); if admitted == () || admitted.status == "pending" { return; }
        if admitted.ok != true || admitted.data == () || admitted.data.experiment_id == () {
            this = finish(this, ["batch admission rejected: " + admitted.to_string()]); return;
        }
        this.run = admitted.data.experiment_id; this.phase = "complete";
    }
    if this.phase != "complete" { return; }
    let state = query("RunStatus", #{experiment_id: this.run});
    if state == () || state.ok == false { return; }
    if state.state == "failed" || state.state == "cancelled" { this = finish(this, ["valid successor failed: " + state.to_string()]); return; }
    if state.state != "done" { return; }
    let current = result(this.run);
    let expected = if this.mode == "seed" { 1.0 } else { 3.0 };
    let failures = [
        t_true(values_are(current,expected), "current source completes with its own values"),
        t_true(current.source_cid != () && current.restored_history == false, "completion carries actual compiled source identity")
    ];
    if this.mode == "restore" {
        let archived = result(this.seed_id);
        failures.push(t_true(this.run != this.seed_id && current.source_cid != this.seed_cid, "same-name changed source has a distinct run and source CID"));
        failures.push(t_true(values_are(archived,1.0) && archived.source_cid == this.seed_cid && archived.restored_history == true, "successor cannot retag or overwrite archived source identity"));
    }
    this = finish(this, failures);
}
