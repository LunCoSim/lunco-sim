// @timing presentation
// Twin-scoped UI lifetime and event delivery without a scene payload.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectWorkbench"], required_inputs: []}
}
fn workbench() { query("InspectWorkbench") }
fn tour() { cmd_data("InspectGuidedHud", #{}).tour }
fn record(state, condition, label) {
    state.checks.push(t_true(condition, label));
    if !condition { state.failure = label; }
    state
}
fn must_cmd(name, params) {
    let result = cmd(name, params);
    if !result.ok { throw name + ": " + result.error; }
    result
}
fn finish(state, failure) {
    if state.done { return state; }
    if failure != "" { state.checks.push(t_true(false, failure)); }
    state.done = true;
    report_verdict(state.checks, "Scene-free workbench", "WORKBENCH_SCENE_FREE");
    state
}
fn valid(me) { this.failure == "" }
fn ready(me) {
    let state = workbench();
    state != () && !state.preparing && !state.scene_mounted
        && state.error != () && state.error.contains("missing-panel")
        && execution_context().scope == "application"
}
fn start_valid_twin(me) {
    this = record(this, !workbench().scene_mounted && execution_context().scope == "application", "an application observer starts without a scene");
    this.previous_twin = workbench().twin_id;
    must_cmd("OpenTwin", #{path: this.good_twin});
    this.waiting_for = "admitting the no-scene Twin";
}
fn valid_twin_ready(me) {
    let state = workbench();
    state.twin_id != this.previous_twin && state.error == () && !state.scene_mounted
}
fn start_tour(me) {
    let state = workbench();
    this.twin = state.twin_id;
    this = record(this, state.error == () && !state.scene_mounted, "Twin capabilities and perspective policies admit without a scene");
    must_cmd("RunScenarioAsset", #{source_asset: "lunco://tutorials/perspectives/overview.rhai", owner_twin_id: this.twin});
    this.waiting_for = "the Twin-owned navigation tour";
}
fn tour_started(me) { tour() != () }
fn inspect_tour_started(me) {
    let state = workbench(); let guide = tour();
    this = record(this, guide.index == 0 && guide.twin == this.twin && !state.scene_mounted, "a shipped navigation tour starts under the exact Twin with no scene");
    must_cmd("ActivatePerspective", #{id: "sandbox_view"});
    must_cmd("GuidedNext", #{});
    this.waiting_for = "the first no-scene tour step";
}
fn first_step_ready(me) {
    let guide = tour();
    guide != () && guide.index == 1 && workbench().active_perspective == "sandbox_view"
}
fn inspect_first_step(me) {
    this = record(this, tour().twin == this.twin, "Next reaches the presentation program while the scene gate is closed");
    must_cmd("GuidedNext", #{});
    this.waiting_for = "the second no-scene tour step";
}
fn second_step_ready(me) { tour() != () && tour().index == 2 }
fn return_to_first_step(me) {
    must_cmd("GuidedBack", #{});
    this.waiting_for = "Back returning to the first no-scene tour step";
}
fn first_step_restored(me) { tour() != () && tour().index == 1 }
fn inspect_back_step(me) {
    this = record(this, !workbench().scene_mounted, "Back advances the same scene-independent presentation clock");
    print("SCENE_FREE_TOUR_CAPTURE");
}
fn replace_with_invalid_twin(me) {
    must_cmd("OpenTwin", #{path: this.bad_twin});
    this.waiting_for = "retiring the tour and rejecting the invalid Twin";
}
fn invalid_twin_ready(me) {
    let state = workbench();
    state.twin_id != this.twin && state.error != () && !state.scene_mounted
}
fn verify_retirement(me) {
    let state = workbench(); let guide = tour();
    this = record(this, guide == (), "Twin replacement retires the outgoing coach presentation");
    this = record(this, !state.scene_mounted && state.error.contains("missing-panel"), "an invalid custom layout reports its error without creating a scene");
    must_cmd("GuidedNext", #{});
    this.waiting_for = "the retired tour remaining absent";
}
fn retired_program_stays_absent(me) {
    tour() == () && execution_context().scope == "application" && !workbench().scene_mounted
}
fn finish_retirement(me) {
    this = record(this, tour() == () && execution_context().scope == "application", "a retired Twin program cannot consume later navigation or recreate its tour");
    this = finish(this, "");
}
fn timeout(me) {
    let message = if this.failure != "" { this.failure }
        else { "scene-free workbench behavior tree timed out waiting for " + this.waiting_for };
    this = finish(this, message);
}
fn on_start(me, ctx) {
    this.done = false; this.failure = ""; this.checks = [];
    this.waiting_for = "the rejected scene-free custom layout";
    this.good_twin = ctx.good_twin; this.bad_twin = ctx.bad_twin;
    subscribe("TASK_FAILED");
}
fn on_event(me, event, ctx) {
    if event.name == "TASK_FAILED" { this = finish(this, "scene-free behavior tree task failed: " + this.failure); }
}
fn on_stop(me, ctx) {
    if !this.done { this = finish(this, "scene-free behavior tree stopped before its verdict while waiting for " + this.waiting_for); }
}
fn task(me, ctx) {
    par_race([
        seq([
            wait_until(Fn("ready")), once(Fn("start_valid_twin")), check(Fn("valid")),
            wait_until(Fn("valid_twin_ready")), once(Fn("start_tour")), check(Fn("valid")),
            wait_until(Fn("tour_started")), once(Fn("inspect_tour_started")), check(Fn("valid")),
            wait_until(Fn("first_step_ready")), once(Fn("inspect_first_step")), check(Fn("valid")),
            wait_until(Fn("second_step_ready")), once(Fn("return_to_first_step")), wait_until(Fn("first_step_restored")),
            once(Fn("inspect_back_step")), check(Fn("valid")), wait(1.25), once(Fn("replace_with_invalid_twin")),
            wait_until(Fn("invalid_twin_ready")), once(Fn("verify_retirement")), check(Fn("valid")),
            wait_until(Fn("retired_program_stays_absent")), once(Fn("finish_retirement")),
        ]),
        seq([wait(45.0), once(Fn("timeout"))]),
    ])
}
