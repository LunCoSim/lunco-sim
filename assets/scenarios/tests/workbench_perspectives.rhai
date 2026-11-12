// Production workbench admission and Twin-lifecycle acceptance.
// @timing presentation
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectWorkbench"], required_inputs: []}
}
fn workbench() { query("InspectWorkbench") }
fn contains_id(items, id) {
    for item in items { if item.id == id { return true; } }
    false
}
fn rendered(state, id) {
    for panel in state.rendered_panels { if panel.id == id { return panel; } }
    ()
}
fn must_cmd(name, params) {
    let result = cmd(name, params);
    if !result.ok { throw name + ": " + result.error; }
    result
}
fn record(state, condition, label) {
    state.checks.push(t_true(condition, label));
    if !condition { state.failure = label; }
    state
}
fn finish(state, failure) {
    if state.done { return state; }
    if failure != "" { state.checks.push(t_true(false, failure)); }
    state.done = true;
    report_verdict(state.checks, "Workbench perspectives", "WORKBENCH_PERSPECTIVES");
    state
}
fn valid(me) { this.failure == "" }
fn ready(me) {
    let state = workbench();
    if state == () || state.preparing || !state.scene_mounted || state.error != () { return false; }
    let required = contains_id(state.perspectives, "rover_build")
        && contains_id(state.perspectives, "editor")
        && contains_id(state.perspectives, "modelica_analyze");
    let graph = false;
    for panel in state.instance_panels { if panel.id == "modelica_plot" { graph = true; } }
    required && graph
}
fn begin(me) {
    let state = workbench();
    for panel in state.instance_panels {
        if panel.id == "modelica_plot" {
            this.plot_kind = panel.id;
            this.plot_instance = panel.canonical_instance;
        }
    }
    this = record(this, contains_id(state.perspectives, "rover_build") && contains_id(state.perspectives, "editor") && contains_id(state.perspectives, "modelica_analyze"), "application policy assembled the engineering tasks");
    let rejected = cmd("OpenTab", #{kind: "missing-panel-kind", instance: 0});
    this = record(this, !rejected.ok && rejected.error.contains("missing-panel-kind"), "unknown instance kinds reject visibly without opening a tab");
    must_cmd("ActivatePerspective", #{id: "modelica_analyze"});
    must_cmd("ResetWorkspaceLayout", #{});
    this.waiting_for = "the Modelica Analyze workspace";
}
fn analyze_workspace_ready(me) {
    let state = workbench();
    state.active_perspective == "modelica_analyze"
        && state.docked_panels.contains("modelica_workspace")
        && !state.docked_panels.contains(this.plot_kind)
}
fn inspect_workspace_default(me) {
    this = record(this, workbench().docked_panels.contains("modelica_workspace") && !workbench().docked_panels.contains(this.plot_kind), "Lunica keeps its model workspace central and opens Graphs on demand");
}
fn open_graph(me) {
    must_cmd("OpenTab", #{kind: this.plot_kind, instance: this.plot_instance});
    print("PERSPECTIVES_CAPTURE:modelica-graphs-open");
    this.waiting_for = "Graphs beside the Modelica workspace";
}
fn graph_open(me) { rendered(workbench(), this.plot_kind) != () }
fn inspect_graph_dock(me) {
    let state = workbench();
    let graph = rendered(state, this.plot_kind);
    let workspace = rendered(state, "modelica_workspace");
    if workspace == () { workspace = rendered(state, "usd::preview_view"); }
    if workspace == () { workspace = rendered(state, "workbench::viewport"); }
    let positioned = false;
    if graph != () && workspace != () {
        positioned = graph.width >= workspace.width
            && graph.left <= workspace.left
            && graph.left + graph.width >= workspace.left + workspace.width
            && graph.top >= workspace.top + workspace.height;
    }
    this = record(this, positioned && state.docked_panels.contains("modelica_workspace"),
        "Graphs opens below the central work area while the Modelica workspace remains open");
    must_cmd("CloseTab", #{kind: this.plot_kind, instance: this.plot_instance});
    this.waiting_for = "closing Graphs while keeping the workspace";
}
fn graph_closed(me) { !workbench().docked_panels.contains(this.plot_kind) }
fn verify_graph_closed(me) {
    this = record(this, workbench().docked_panels.contains("modelica_workspace"), "closing Graphs leaves the central Modelica workspace in place");
    must_cmd("ActivatePerspective", #{id: "editor"});
    must_cmd("ResetWorkspaceLayout", #{});
    this.waiting_for = "the empty Editor layout";
}
fn editor_ready(me) { workbench().active_perspective == "editor" }
fn inspect_empty_editor(me) {
    let state = workbench();
    this = record(this, !state.docked_panels.contains("usd_connection_canvas") && !state.docked_panels.contains("rhai_editor") && !state.docked_panels.contains("usd::viewport"), "Editor starts without content tabs");
    print("PERSPECTIVES_CAPTURE:editor-empty");
}
fn focus_connections(me) {
    must_cmd("FocusPanel", #{id: "usd_connection_canvas"});
    this.waiting_for = "Connections opening in Editor";
}
fn connections_visible(me) { rendered(workbench(), "usd_connection_canvas") != () }
fn inspect_connections(me) {
    this = record(this, workbench().docked_panels.contains("usd_connection_canvas"), "explicit tool requests open Connections in Editor");
    must_cmd("ResetWorkspaceLayout", #{});
    this.waiting_for = "the reset empty Editor layout";
}
fn connections_closed(me) {
    let state = workbench();
    !state.docked_panels.contains("usd_connection_canvas")
        && !state.docked_panels.contains("rhai_editor")
        && !state.docked_panels.contains("usd::viewport")
}
fn verify_editor_reset(me) {
    this = record(this, true, "explicit reset restores the empty Editor center");
    must_cmd("ActivatePerspective", #{id: "rover_build"});
    this.waiting_for = "the Build perspective";
}
fn build_ready(me) { workbench().active_perspective == "rover_build" }
fn inspect_build(me) {
    let state = workbench();
    this = record(this, !state.docked_panels.contains("telemetry_browser") && !state.docked_panels.contains("modelica_plot"), "Build concentrates on composition");
    print("PERSPECTIVES_CAPTURE:build");
}
fn activate_analysis(me) {
    must_cmd("ActivatePerspective", #{id: "analysis"});
    must_cmd("ResetWorkspaceLayout", #{});
    this.waiting_for = "the default Analyze panels";
}
fn analysis_ready(me) { workbench().active_perspective == "analysis" }
fn analyze_panels_ready(me) {
    let state = workbench();
    rendered(state, "modelica_plot") != ()
        && rendered(state, "telemetry_browser") != ()
        && rendered(state, "workbench::viewport") != ()
}
fn inspect_analyze(me) {
    let state = workbench();
    let graph = rendered(state, "modelica_plot");
    let telemetry = rendered(state, "telemetry_browser");
    let viewport = rendered(state, "workbench::viewport");
    this = record(this,
        graph.width > telemetry.width
            && graph.top >= viewport.top + viewport.height
            && viewport.height > graph.height
            && abs(graph.left - viewport.left) < 2.0,
        "Analyze keeps the live scene above its bottom graph");
    this = record(this, state.docked_panels.contains("modelica_plot") && state.docked_panels.contains("telemetry_browser") && state.docked_panels.contains("port_inspector"), "Analyze consumes the real graph, telemetry and port capabilities");
    print("PERSPECTIVES_CAPTURE:analyze");
}
fn focus_joint_state(me) {
    must_cmd("FocusPanel", #{id: "sandbox_joint_state"});
    this.waiting_for = "the joint-state tool";
}
fn joint_state_open(me) { workbench().docked_panels.contains("sandbox_joint_state") }
fn close_graph(me) {
    must_cmd("CloseTab", #{kind: this.plot_kind, instance: this.plot_instance});
    this.waiting_for = "closing the canonical graph";
}
fn graph_reopened(me) {
    let state = workbench();
    rendered(state, this.plot_kind) != ()
        && rendered(state, "telemetry_browser") != ()
        && rendered(state, "port_inspector") != ()
        && rendered(state, "workbench::viewport") != ()
}
fn reopen_graph(me) {
    must_cmd("OpenTab", #{kind: this.plot_kind, instance: this.plot_instance});
    this.waiting_for = "reopening Graphs with the other Analyze tools";
}
fn inspect_reopened_graph(me) {
    let state = workbench();
    let graph = rendered(state, this.plot_kind);
    let viewport = rendered(state, "workbench::viewport");
    this = record(this,
        graph.width >= viewport.width
            && graph.left <= viewport.left
            && graph.left + graph.width >= viewport.left + viewport.width
            && graph.top >= viewport.top + viewport.height
            && state.docked_panels.contains("sandbox_joint_state"),
        "reopening the canonical graph restores its bottom dock and preserves other tools");
    print("PERSPECTIVES_CAPTURE:reopened-graph");
}
fn select_analysis_again(me) { must_cmd("ActivatePerspective", #{id: "analysis"}); }
fn inspect_idempotent_analysis(me) {
    let state = workbench();
    this = record(this, state.docked_panels.contains("sandbox_joint_state") && state.docked_panels.contains(this.plot_kind), "selecting the active task is idempotent");
    print("PERSPECTIVES_POLICY_STIMULUS:reload");
    this.waiting_for = "the reloaded Analyze policy";
}
fn reloaded_policy_ready(me) {
    for item in workbench().perspectives { if item.id == "analysis" && item.title == "Analysis reloaded" { return true; } }
    false
}
fn inspect_reloaded_policy(me) {
    let state = workbench();
    this = record(this, state.error == () && state.active_perspective == "analysis", "the replaced Rhai composition policy is admitted without changing the active task");
    must_cmd("ActivatePerspective", #{id: "rover_build"});
    this.waiting_for = "navigating away from the reloaded task";
}
fn build_after_reload_ready(me) { workbench().active_perspective == "rover_build" }
fn return_to_analysis(me) {
    must_cmd("ActivatePerspective", #{id: "analysis"});
    this.waiting_for = "restoring the reloaded Analyze layout";
}
fn restored_analysis_layout_ready(me) {
    let state = workbench();
    state.active_perspective == "analysis" && rendered(state, "modelica_plot") != ()
}
fn inspect_restored_analysis(me) {
    this = record(this, workbench().docked_panels.contains("sandbox_joint_state"), "a policy with changed initial instances preserves a visited dock and its bottom graph across navigation");
    print("PERSPECTIVES_POLICY_STIMULUS:restore");
    this.waiting_for = "restoring the application perspective defaults";
}
fn default_policy_ready(me) {
    let state = workbench();
    state.error == () && !has_reloaded_title(state)
}
fn has_reloaded_title(state) {
    for item in state.perspectives { if item.id == "analysis" && item.title == "Analysis reloaded" { return true; } }
    false
}
fn inspect_default_policy(me) {
    this = record(this, workbench().error == () && contains_id(workbench().perspectives, "editor"), "Rhai restores the exact application defaults through the same owner");
    print("PERSPECTIVES_POLICY_STIMULUS:reject");
    this.waiting_for = "rejecting the invalid nested perspective field";
}
fn rejected_policy_ready(me) { workbench().error != () }
fn inspect_rejected_policy(me) {
    let state = workbench();
    this = record(this, state.error.contains("position") && state.active_perspective == "analysis" && state.docked_panels.contains("sandbox_joint_state"), "unknown nested layout fields reject visibly and preserve the admitted dock");
    print("PERSPECTIVES_POLICY_STIMULUS:recover");
    this.waiting_for = "recovering the valid application defaults";
}
fn recovered_policy_ready(me) { workbench().error == () }
fn activate_build_again(me) {
    must_cmd("ActivatePerspective", #{id: "rover_build"});
    this.waiting_for = "returning to the application Build task";
}
fn activate_analysis_again(me) {
    must_cmd("ActivatePerspective", #{id: "analysis"});
    this.waiting_for = "returning to the visited Analyze task";
}
fn visited_analysis_ready(me) { workbench().active_perspective == "analysis" }
fn reset_analysis(me) {
    this = record(this, workbench().docked_panels.contains("sandbox_joint_state"), "returning to a visited task preserves the user's open tools");
    must_cmd("ResetWorkspaceLayout", #{});
    this.waiting_for = "resetting the visited Analyze layout";
}
fn analysis_reset_ready(me) { !workbench().docked_panels.contains("sandbox_joint_state") }
fn open_good_twin(me) {
    this = record(this, workbench().docked_panels.contains(this.plot_kind), "explicit reset reapplies the current Rhai layout");
    this.previous_twin = workbench().twin_id;
    must_cmd("OpenTwin", #{path: this.good_twin});
    this.waiting_for = "opening the valid Twin";
}
fn good_twin_ready(me) {
    let state = workbench();
    state.twin_id != this.previous_twin && contains_id(state.perspectives, "twin/power-study")
}
fn activate_twin_task(me) {
    this.good_id = workbench().twin_id;
    this = record(this, workbench().error == (), "a shipped Twin policy is admitted without UI errors");
    must_cmd("ActivatePerspective", #{id: "twin/power-study"});
    this.waiting_for = "the Twin power-study task";
}
fn twin_task_ready(me) { workbench().active_perspective == "twin/power-study" }
fn inspect_twin_task(me) {
    let state = workbench();
    this = record(this, state.docked_panels.contains("modelica_plot") && state.docked_panels.contains("port_inspector"), "a Twin task materializes the shared capabilities");
    print("PERSPECTIVES_CAPTURE:twin-power-study");
}
fn open_invalid_twin(me) {
    must_cmd("OpenTwin", #{path: this.bad_twin});
    this.waiting_for = "retiring the valid Twin and rejecting the invalid Twin layout";
}
fn invalid_twin_ready(me) {
    let state = workbench();
    state.twin_id != this.good_id && state.error != ()
}
fn inspect_invalid_twin(me) {
    let state = workbench();
    this.bad_id = state.twin_id;
    this = record(this, !contains_id(state.perspectives, "twin/power-study") && !contains_id(state.perspectives, "twin/invalid"), "replacement retires outgoing names and invalid plans install no custom layout");
    this = record(this, state.error.contains("missing-panel") && contains_id(state.perspectives, "analysis"), "missing capabilities fail visibly while application tasks remain usable");
    must_cmd("ActivatePerspective", #{id: "analysis"});
    this.waiting_for = "using application Analyze after invalid Twin admission";
}
fn application_analysis_ready(me) { workbench().active_perspective == "analysis" }
fn verify_analysis_after_rejection(me) {
    this = record(this, workbench().docked_panels.contains("modelica_plot"), "a rejected custom layout does not crash or disable analysis");
    must_cmd("OpenTwin", #{path: this.good_twin});
    this.waiting_for = "reopening the valid Twin with a fresh lease";
}
fn reopened_good_twin_ready(me) {
    let state = workbench();
    state.twin_id != this.bad_id && contains_id(state.perspectives, "twin/power-study")
}
fn reopen_twin_task(me) {
    this = record(this, workbench().error == () && workbench().twin_id != this.good_id, "A to B to A receives a fresh Twin lease and fresh policy definitions");
    must_cmd("ActivatePerspective", #{id: "twin/power-study"});
    this.waiting_for = "checking the reopened Twin task's transient panels";
}
fn finish_reopened_twin(me) {
    this = record(this, !workbench().docked_panels.contains("sandbox_joint_state"), "reopening a Twin does not revive a retired task's transient tools");
    this = finish(this, "");
}
fn timeout(me) {
    let message = if this.failure != "" { this.failure }
        else { "workbench perspective behavior tree timed out waiting for " + this.waiting_for };
    this = finish(this, message);
}
fn on_start(me, ctx) {
    this.done = false; this.failure = ""; this.checks = [];
    this.waiting_for = "the mounted workbench";
    this.good_twin = ctx.good_twin; this.bad_twin = ctx.bad_twin;
    subscribe("TASK_FAILED");
}
fn on_event(me, event, ctx) {
    if event.name == "TASK_FAILED" { this = finish(this, "perspective behavior tree task failed: " + this.failure); }
}
fn on_stop(me, ctx) {
    if !this.done { this = finish(this, "perspective behavior tree stopped before its verdict while waiting for " + this.waiting_for); }
}
fn task(me, ctx) {
    par_race([
        seq([
            wait_until(Fn("ready")), once(Fn("begin")), check(Fn("valid")),
            wait_until(Fn("analyze_workspace_ready")), once(Fn("inspect_workspace_default")), check(Fn("valid")), wait(1.5),
            once(Fn("open_graph")), wait(1.5), wait_until(Fn("graph_open")),
            once(Fn("inspect_graph_dock")), check(Fn("valid")), wait_until(Fn("graph_closed")),
            once(Fn("verify_graph_closed")), check(Fn("valid")), wait_until(Fn("editor_ready")),
            once(Fn("inspect_empty_editor")), check(Fn("valid")), wait(1.5),
            once(Fn("focus_connections")), wait_until(Fn("connections_visible")), once(Fn("inspect_connections")), check(Fn("valid")),
            wait_until(Fn("connections_closed")), once(Fn("verify_editor_reset")), check(Fn("valid")), wait_until(Fn("build_ready")),
            once(Fn("inspect_build")), check(Fn("valid")), wait(1.5), once(Fn("activate_analysis")), wait_until(Fn("analysis_ready")),
            wait_until(Fn("analyze_panels_ready")), once(Fn("inspect_analyze")), check(Fn("valid")), wait(1.5),
            once(Fn("focus_joint_state")), wait_until(Fn("joint_state_open")), once(Fn("close_graph")), wait_until(Fn("graph_closed")),
            once(Fn("reopen_graph")), wait_until(Fn("graph_reopened")), once(Fn("inspect_reopened_graph")), check(Fn("valid")), wait(1.5),
            once(Fn("select_analysis_again")), wait_until(Fn("analysis_ready")), once(Fn("inspect_idempotent_analysis")), check(Fn("valid")),
            wait_until(Fn("reloaded_policy_ready")), once(Fn("inspect_reloaded_policy")), check(Fn("valid")), wait_until(Fn("build_after_reload_ready")),
            once(Fn("return_to_analysis")), wait_until(Fn("restored_analysis_layout_ready")), once(Fn("inspect_restored_analysis")), check(Fn("valid")),
            wait_until(Fn("default_policy_ready")), once(Fn("inspect_default_policy")), check(Fn("valid")),
            wait_until(Fn("rejected_policy_ready")), once(Fn("inspect_rejected_policy")), check(Fn("valid")), wait_until(Fn("recovered_policy_ready")),
            once(Fn("activate_build_again")), wait_until(Fn("build_ready")), once(Fn("activate_analysis_again")), wait_until(Fn("visited_analysis_ready")),
            once(Fn("reset_analysis")), check(Fn("valid")), wait_until(Fn("analysis_reset_ready")), once(Fn("open_good_twin")), check(Fn("valid")),
            wait_until(Fn("good_twin_ready")), once(Fn("activate_twin_task")), check(Fn("valid")), wait_until(Fn("twin_task_ready")),
            once(Fn("inspect_twin_task")), check(Fn("valid")), wait(1.5), once(Fn("open_invalid_twin")),
            wait_until(Fn("invalid_twin_ready")), once(Fn("inspect_invalid_twin")), check(Fn("valid")), wait_until(Fn("application_analysis_ready")),
            once(Fn("verify_analysis_after_rejection")), check(Fn("valid")), wait_until(Fn("reopened_good_twin_ready")),
            once(Fn("reopen_twin_task")), check(Fn("valid")), wait_until(Fn("twin_task_ready")), once(Fn("finish_reopened_twin")),
        ]),
        seq([wait(60.0), once(Fn("timeout"))]),
    ])
}
