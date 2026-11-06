// @timing presentation
// The REPL harness owns settings changes; this observer checks their public results.
fn install_profile(marker) {
    let value = if marker == "high" { 16 }
        else if marker == "maximum" || marker == "same" { 64 }
        else if marker == "restore" || marker == "final" { 8 }
        else { throw "unknown solver-profile stimulus: " + marker; };
    if !set_physics_substeps(value) { throw "valid solver profile rejected"; }
}
fn verify_invalid_profile() {
    let before = physics_substeps();
    report_verdict([
        t_true(!set_physics_substeps(0), "zero substeps remain invalid"),
        t_true(!set_physics_substeps(65), "the diagnostic range remains bounded"),
        t_true(physics_substeps() == before, "rejection preserves the admitted profile"),
        t_true(cmd("RunLint", #{domain: "physics"}).ok, "explicit lint uses the live solver producer"),
        t_true(!cmd("RunLint", #{domain: "physics", doc_id: 1}).ok,
            "a document-scoped request cannot silently lint the live solver")
    ], "Invalid solver profile", "PHYSICS_SOLVER_PROFILE_INVALID");
}
fn profile_finding() {
    for finding in query("GetDiagnostics", #{scope: "physics"}).diagnostics {
        if finding.code == "physics-substeps-above-standard" { return finding; }
    }
    ()
}
fn on_start(me, ctx) {
    this.phase = "high";
    this.events = 0;
    this.event_error = false;
    this.updates = 0;
    this.checks = [];
    this.start = clock_snapshot().sim_tick;
    this.done = false;
    print("PHYSICS_PROFILE_STIMULUS:high");
}
fn on_event(me, evt, ctx) {
    if evt.name == "physics.solver-profile"
        && evt.value.contains("Potential performance issue") {
        this.events += 1;
        this.event_error = evt.severity == "Error";
    }
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.updates += 1;
    if this.updates > 2400 { throw "solver profile lint did not settle"; }
    let finding = profile_finding();
    if this.phase == "high" && physics_substeps() == 16 && finding != ()
        && this.events > 0 && clock_snapshot().sim_tick >= this.start + 3 {
        this.checks.push(t_true(finding.severity == "error", "above-standard resolution is a lint error"));
        this.checks.push(t_true(finding.message.contains("Potential performance issue"), "the finding describes a potential performance issue"));
        this.checks.push(t_true(this.events == 1 && this.event_error, "the error reaches the event feed consumed by Recent Events once"));
        this.checks.push(t_true(!query("GetReadiness").faulted, "lint does not fault the running simulation"));
        this.checks.push(t_true(clock_snapshot().sim_tick > this.start, "the higher substep setting continues to simulate"));
        print("PHYSICS_PROFILE_STIMULUS:restore"); this.phase = "restore"; return;
    }
    if this.phase == "restore" && physics_substeps() == 8 && finding == () {
        this.checks.push(t_true(query("GetDiagnostics", #{scope:"physics"}).ok, "returning to standard clears the current lint"));
        print("PHYSICS_PROFILE_STIMULUS:maximum"); this.phase = "maximum"; return;
    }
    if this.phase == "maximum" && physics_substeps() == 64 && finding != () && this.events >= 2 {
        this.checks.push(t_true(finding.message.contains("64"), "the supported high diagnostic value is retained and reported"));
        this.checks.push(t_true(!query("GetReadiness").faulted, "high diagnostic resolution remains advisory"));
        this.same_at = this.updates;
        this.same_revision = query("GetDiagnostics", #{scope: "physics"}).revision;
        print("PHYSICS_PROFILE_STIMULUS:same"); this.phase = "same"; return;
    }
    if this.phase == "same" && this.updates >= this.same_at + 20 {
        this.checks.push(t_true(this.events == 2 && query("GetDiagnostics", #{scope: "physics"}).revision == this.same_revision, "an unchanged profile does not rerun lint or spam Recent Events"));
        print("PHYSICS_PROFILE_STIMULUS:final"); this.phase = "final"; return;
    }
    if this.phase == "final" && physics_substeps() == 8 && finding == () {
        this.checks.push(t_true(!query("GetReadiness").faulted, "restoration preserves runtime admission"));
        this.done = true;
        report_verdict(this.checks, "Physics solver profile lint", "PHYSICS_SOLVER_PROFILE_LINT");
    }
}
