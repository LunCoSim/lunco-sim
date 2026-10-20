// @timing presentation
// Twin-scoped UI lifetime and event delivery without a scene payload.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectWorkbench"], required_inputs: []}
}
fn on_start(me, ctx) {
    this.phase = 0; this.observed_phase = -1; this.phase_elapsed = 0.0; this.elapsed = 0.0; this.done = false; this.checks = [];
}
fn finish(checks) { report_verdict(checks, "Scene-free workbench", "WORKBENCH_SCENE_FREE"); }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += execution_context().delta_seconds;
    if this.phase != this.observed_phase { this.observed_phase = this.phase; this.phase_elapsed = 0.0; }
    this.phase_elapsed += execution_context().delta_seconds;
    if this.phase_elapsed > 10.0 {
        this.checks.push(t_true(false, "scene-free workflow timed out in phase " + this.phase));
        this.done = true; finish(this.checks); return;
    }
    let state = query("InspectWorkbench");
    if state == () || state.preparing { return; }
    if this.phase == 0 {
        if state.error == () || !state.error.contains("missing-panel") { return; }
        this.checks.push(t_true(!state.scene_mounted && execution_context().scope == "application", "an application observer starts without a scene"));
        this.previous_twin = state.twin_id;
        cmd("OpenTwin", #{path: ctx.good_twin}); this.phase = 1; return;
    }
    if this.phase == 1 {
        if state.twin_id == this.previous_twin { return; }
        this.twin = state.twin_id;
        this.checks.push(t_true(state.error == () && !state.scene_mounted, "Twin capabilities and perspective policies admit without a scene"));
        cmd("RunScenarioAsset", #{source_asset: "lunco://tutorials/perspectives/overview.rhai", owner_twin_id: this.twin});
        this.phase = 2; return;
    }
    let tour = cmd_data("InspectGuidedHud", #{}).tour;
    if this.phase == 2 {
        if tour == () { return; }
        this.checks.push(t_true(tour.index == 0 && tour.twin == this.twin && !state.scene_mounted, "a shipped navigation tour starts under the exact Twin with no scene"));
        cmd("ActivatePerspective", #{id: "sandbox_view"});
        cmd("GuidedNext", #{}); this.phase = 3; return;
    }
    if this.phase == 3 {
        if tour == () || tour.index != 1 || state.active_perspective != "sandbox_view" { return; }
        this.checks.push(t_true(tour.twin == this.twin, "Next reaches the presentation program while the scene gate is closed"));
        cmd("GuidedNext", #{}); this.phase = 4; return;
    }
    if this.phase == 4 {
        if tour == () || tour.index != 2 { return; }
        cmd("GuidedBack", #{}); this.phase = 5; return;
    }
    if this.phase == 5 {
        if tour == () || tour.index != 1 { return; }
        this.checks.push(t_true(!state.scene_mounted, "Back advances the same scene-independent presentation clock"));
        this.captured = false; this.capture_at = this.elapsed;
        this.phase = 6; return;
    }
    if this.phase == 6 {
        if this.elapsed - this.capture_at < 0.5 { return; }
        if !this.captured { print("SCENE_FREE_TOUR_CAPTURE"); this.captured = true; }
        if this.elapsed - this.capture_at < 1.25 { return; }
        cmd("OpenTwin", #{path: ctx.bad_twin}); this.phase = 7; return;
    }
    if this.phase == 7 {
        if state.twin_id == this.twin || state.error == () { return; }
        this.checks.push(t_true(tour == (), "Twin replacement retires the outgoing coach presentation"));
        this.checks.push(t_true(!state.scene_mounted && state.error.contains("missing-panel"), "an invalid custom layout reports its error without creating a scene"));
        cmd("GuidedNext", #{}); this.phase = 8; return;
    }
    this.checks.push(t_true(tour == () && execution_context().scope == "application", "a retired Twin program cannot consume later navigation or recreate its tour"));
    this.done = true; finish(this.checks);
}
