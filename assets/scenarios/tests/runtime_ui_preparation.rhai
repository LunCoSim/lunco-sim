// @timing presentation
// Public preparation state, caller clock, failure and recovery while simulation is held.
fn policy_source(marker) {
    "fn project(facts) { #{visible: " + (marker != "hidden").to_string() + ", properties: #{marker: \"" + marker + "\", clock: runtime_context.clock, cycle: runtime_context.cycle, phase: runtime_context.phase}} }"
}
// The harness invokes this through the ordinary privileged REPL boundary.
// Scenario actors observe the result; they cannot replace application policy.
fn install_policy(marker) {
    let source = if marker == "invalid" {
        `fn project(facts) { #{visible: true, properties: "invalid view model"} }`
    } else if marker == "invalid-visible" {
        `fn project(facts) { #{visible: "invalid", properties: #{marker: "partial"}} }`
    } else { policy_source(marker) };
    let result = bind_policy("runtime.ui.surface", "project", source);
    if result.ok != true { throw "test policy binding was rejected: " + result; }
}
fn on_start(me, ctx) {
    this.done = false;
    this.phase = "first";
    this.updates = 0;
    this.checks = [];
    cmd("StepSimulation", #{hold: true});
    print("UI_POLICY_TEST_STIMULUS:first");
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.updates += 1;
    let view = query("ReadExposures", #{surface: "camera-status"}).surfaces["camera-status"];
    if view != () && view.state == () {
        this.done = true;
        report_verdict(["prepared exposures must expose a typed publication state"], "Runtime UI preparation", "RUNTIME_UI_PREPARATION");
        return;
    }
    if this.updates > 2400 {
        this.done = true;
        report_verdict(["UI preparation did not complete while simulation was held"], "Runtime UI preparation", "RUNTIME_UI_PREPARATION");
        return;
    }
    if view == () { return; }
    if this.phase == "first" && view.state == "ready" && view.properties.marker == "first" {
        this.tick = clock_snapshot().sim_tick;
        this.checks.push(t_true(clock_snapshot().manual_simulation_hold, "the simulation hold must remain authoritative"));
        this.checks.push(t_true(view.properties.clock == "presentation" && view.properties.cycle == "presentation", "camera preparation preserves its caller clock and cycle"));
        this.checks.push(t_true(view.properties.phase == "event", "policy replacement carries the owner's event phase"));
        print("UI_POLICY_TEST_STIMULUS:invalid");
        this.phase = "invalid";
        return;
    }
    if this.phase == "invalid" && view.state == "failed" {
        this.checks.push(t_true(view.error != () && view.error.contains("must return"), "policy result rejection exposes its owner diagnostic"));
        this.checks.push(t_true(view.visible == false && view.properties.len() == 0, "a failed projection publishes no partial view model"));
        this.checks.push(t_true(clock_snapshot().sim_tick == this.tick, "UI policy failure does not advance held physics"));
        this.checks.push(t_true(query("GetReadiness").faulted == false, "optional UI preparation cannot fault the simulation"));
        this.previous_error = view.error;
        print("UI_POLICY_TEST_STIMULUS:invalid-visible");
        this.phase = "invalid-visible";
        return;
    }
    if this.phase == "invalid-visible" && view.state == "failed" && view.error != this.previous_error {
        this.checks.push(t_true(view.error.contains("boolean visible"), "invalid visibility is rejected at its owner"));
        this.checks.push(t_true(!view.visible && view.properties.len() == 0, "invalid visibility cannot publish partial properties"));
        print("UI_POLICY_TEST_STIMULUS:hidden");
        this.phase = "hidden";
        return;
    }
    if this.phase == "hidden" && view.state == "ready" && view.properties.marker == "hidden" {
        this.checks.push(t_true(!view.visible, "one Rhai decision controls visibility and properties together"));
        this.checks.push(t_true(clock_snapshot().sim_tick == this.tick, "hidden projection preserves the held simulation clock"));
        print("UI_POLICY_TEST_STIMULUS:recovered");
        this.phase = "recovered";
        return;
    }
    if this.phase == "recovered" && view.state == "ready" && view.properties.marker == "recovered" {
        this.checks.push(t_true(view.error == (), "a new policy generation clears its retired failure"));
        this.checks.push(t_true(view.visible && clock_snapshot().sim_tick == this.tick, "UI recovery completes independently of held physics"));
        this.done = true;
        cmd("StepSimulation", #{hold: false});
        report_verdict(this.checks, "Runtime UI preparation", "RUNTIME_UI_PREPARATION");
    }
}
