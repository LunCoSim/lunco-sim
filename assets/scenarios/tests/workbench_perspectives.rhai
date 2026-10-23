// Production workbench admission and Twin-lifecycle acceptance.
// @timing presentation
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectWorkbench"], required_inputs: [] }
}
fn contains_id(items, id) {
    for item in items { if item.id == id { return true; } }
    false
}
fn must_cmd(name, params) {
    let result = cmd(name, params);
    if !result.ok { throw name + ": " + result.error; }
    result
}
fn finish(checks) { report_verdict(checks, "Workbench perspectives", "WORKBENCH_PERSPECTIVES"); }
fn on_start(me, ctx) {
    this.phase = 0; this.observed_phase = -1; this.phase_elapsed = 0.0; this.elapsed = 0.0; this.resume_at = 0.0; this.done = false; this.checks = [];
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += execution_context().delta_seconds;
    if this.phase != this.observed_phase { this.observed_phase = this.phase; this.phase_elapsed = 0.0; }
    this.phase_elapsed += execution_context().delta_seconds;
    if this.phase_elapsed > 10.0 {
        this.checks.push(t_true(false, "perspective lifecycle timed out in phase " + this.phase));
        this.done = true; finish(this.checks); return;
    }
    if this.elapsed < this.resume_at { return; }
    let state = query("InspectWorkbench");
    if state == () || state.preparing { return; }
    if this.phase == 0 {
        if !state.scene_mounted || state.error != () || !contains_id(state.perspectives, "analysis") { return; }
        for panel in state.instance_panels {
            if panel.id == "modelica_plot" {
                this.plot_kind = panel.id; this.plot_instance = panel.canonical_instance;
            }
        }
        this.checks.push(t_true(contains_id(state.perspectives, "rover_build") && contains_id(state.perspectives, "editor"), "application policy assembled the engineering tasks"));
        let rejected = cmd("OpenTab", #{kind: "missing-panel-kind", instance: 0});
        this.checks.push(t_true(!rejected.ok && rejected.error.contains("missing-panel-kind"), "unknown instance kinds reject visibly without opening a tab"));
        must_cmd("ActivatePerspective", #{id: "editor"});
        must_cmd("ResetWorkspaceLayout", #{}); this.phase = 16; return;
    }
    if this.phase == 16 {
        if state.active_perspective != "editor" { return; }
        this.checks.push(t_true(!state.docked_panels.contains("usd_connection_canvas") && !state.docked_panels.contains("rhai_editor") && !state.docked_panels.contains("usd::viewport"), "Editor starts without content tabs"));
        print("PERSPECTIVES_CAPTURE:editor-empty"); this.resume_at = this.elapsed + 1.5;
        this.phase = 165; return;
    }
    if this.phase == 165 {
        must_cmd("FocusPanel", #{id: "usd_connection_canvas"}); this.phase = 17; return;
    }
    if this.phase == 17 {
        if !contains_id(state.rendered_panels, "usd_connection_canvas") { return; }
        this.checks.push(t_true(state.docked_panels.contains("usd_connection_canvas"), "explicit tool requests open Connections in Editor"));
        must_cmd("ResetWorkspaceLayout", #{}); this.phase = 18; return;
    }
    if this.phase == 18 {
        this.checks.push(t_true(!state.docked_panels.contains("usd_connection_canvas") && !state.docked_panels.contains("rhai_editor") && !state.docked_panels.contains("usd::viewport"), "explicit reset restores the empty Editor center"));
        must_cmd("ActivatePerspective", #{id: "rover_build"}); this.phase = 1; return;
    }
    if this.phase == 1 {
        if state.active_perspective != "rover_build" { return; }
        this.checks.push(t_true(!state.docked_panels.contains("telemetry_browser") && !state.docked_panels.contains("modelica_plot"), "Build concentrates on composition"));
        print("PERSPECTIVES_CAPTURE:build"); this.resume_at = this.elapsed + 1.5;
        this.phase = 15; return;
    }
    if this.phase == 15 {
        must_cmd("ActivatePerspective", #{id: "analysis"}); this.phase = 2; return;
    }
    if this.phase == 2 {
        if state.active_perspective != "analysis" { return; }
        let graph = (); let telemetry = ();
        for panel in state.rendered_panels {
            if panel.id == "modelica_plot" { graph = panel; }
            if panel.id == "telemetry_browser" { telemetry = panel; }
        }
        if graph == () || telemetry == () { return; }
        this.checks.push(t_true(graph.width > telemetry.width && graph.height > telemetry.height, "the authored central graph consumes the primary work area"));
        this.checks.push(t_true(state.docked_panels.contains("modelica_plot") && state.docked_panels.contains("telemetry_browser") && state.docked_panels.contains("port_inspector"), "Analyze consumes the real graph, telemetry and port capabilities"));
        print("PERSPECTIVES_CAPTURE:analyze"); this.resume_at = this.elapsed + 1.5;
        this.phase = 25; return;
    }
    if this.phase == 25 {
        must_cmd("FocusPanel", #{id: "sandbox_joint_state"}); this.phase = 3; return;
    }
    if this.phase == 3 {
        if !state.docked_panels.contains("sandbox_joint_state") { return; }
        must_cmd("CloseTab", #{kind: this.plot_kind, instance: this.plot_instance});
        this.phase = 35; return;
    }
    if this.phase == 35 {
        if state.docked_panels.contains(this.plot_kind) { return; }
        must_cmd("OpenTab", #{kind: this.plot_kind, instance: this.plot_instance});
        this.phase = 36; return;
    }
    if this.phase == 36 {
        let graph = (); let telemetry = (); let ports = (); let joint = ();
        for panel in state.rendered_panels {
            if panel.id == this.plot_kind { graph = panel; }
            if panel.id == "telemetry_browser" { telemetry = panel; }
            if panel.id == "port_inspector" { ports = panel; }
            if panel.id == "sandbox_joint_state" { joint = panel; }
        }
        if graph == () || telemetry == () || ports == () || joint == () { return; }
        this.checks.push(t_true(graph.left > telemetry.left && graph.left < ports.left && graph.width > telemetry.width && joint.top > graph.top + graph.height * 0.5, "reopening the canonical graph restores the authored center without resetting other tools"));
        print("PERSPECTIVES_CAPTURE:reopened-graph"); this.resume_at = this.elapsed + 1.5;
        this.phase = 365; return;
    }
    if this.phase == 365 {
        must_cmd("ActivatePerspective", #{id: "analysis"}); this.phase = 37; return;
    }
    if this.phase == 37 {
        this.checks.push(t_true(state.docked_panels.contains("sandbox_joint_state") && state.docked_panels.contains(this.plot_kind), "selecting the active task is idempotent"));
        print("PERSPECTIVES_POLICY_STIMULUS:reload");
        this.phase = 30; return;
    }
    if this.phase == 30 {
        let updated = false;
        for item in state.perspectives { if item.id == "analysis" && item.title == "Analysis reloaded" { updated = true; } }
        if !updated { return; }
        this.checks.push(t_true(state.error == () && state.active_perspective == "analysis", "the replaced Rhai composition policy is admitted without changing the active task"));
        must_cmd("ActivatePerspective", #{id: "rover_build"}); this.phase = 32; return;
    }
    if this.phase == 32 {
        if state.active_perspective != "rover_build" { return; }
        must_cmd("ActivatePerspective", #{id: "analysis"}); this.phase = 33; return;
    }
    if this.phase == 33 {
        if state.active_perspective != "analysis" { return; }
        if !contains_id(state.rendered_panels, "modelica_plot") { return; }
        this.checks.push(t_true(state.docked_panels.contains("sandbox_joint_state"), "a policy with changed initial instances preserves a visited dock and its central graph across navigation"));
        print("PERSPECTIVES_POLICY_STIMULUS:restore");
        this.phase = 31; return;
    }
    if this.phase == 31 {
        for item in state.perspectives { if item.id == "analysis" && item.title == "Analysis reloaded" { return; } }
        this.checks.push(t_true(state.error == () && contains_id(state.perspectives, "editor"), "Rhai restores the exact application defaults through the same owner"));
        print("PERSPECTIVES_POLICY_STIMULUS:reject"); this.phase = 34; return;
    }
    if this.phase == 34 {
        if state.error == () { return; }
        this.checks.push(t_true(state.error.contains("position") && state.active_perspective == "analysis" && state.docked_panels.contains("sandbox_joint_state"), "unknown nested layout fields reject visibly and preserve the admitted dock"));
        print("PERSPECTIVES_POLICY_STIMULUS:recover"); this.phase = 38; return;
    }
    if this.phase == 38 {
        if state.error != () { return; }
        must_cmd("ActivatePerspective", #{id: "rover_build"}); this.phase = 4; return;
    }
    if this.phase == 4 {
        if state.active_perspective != "rover_build" { return; }
        must_cmd("ActivatePerspective", #{id: "analysis"}); this.phase = 5; return;
    }
    if this.phase == 5 {
        if state.active_perspective != "analysis" { return; }
        this.checks.push(t_true(state.docked_panels.contains("sandbox_joint_state"), "returning to a visited task preserves the user's open tools"));
        must_cmd("ResetWorkspaceLayout", #{}); this.phase = 6; return;
    }
    if this.phase == 6 {
        if state.docked_panels.contains("sandbox_joint_state") { return; }
        this.checks.push(t_true(state.docked_panels.contains("modelica_plot"), "explicit reset reapplies the current Rhai layout"));
        must_cmd("OpenTwin", #{path: ctx.good_twin}); this.old_twin = state.twin_id; this.phase = 7; return;
    }
    if this.phase == 7 {
        if state.twin_id == this.old_twin || !contains_id(state.perspectives, "twin/power-study") { return; }
        this.good_id = state.twin_id;
        this.checks.push(t_true(state.error == (), "a shipped Twin policy is admitted without UI errors"));
        must_cmd("ActivatePerspective", #{id: "twin/power-study"}); this.phase = 8; return;
    }
    if this.phase == 8 {
        if state.active_perspective != "twin/power-study" { return; }
        this.checks.push(t_true(state.docked_panels.contains("modelica_plot") && state.docked_panels.contains("port_inspector"), "a Twin task materializes the shared capabilities"));
        print("PERSPECTIVES_CAPTURE:twin-power-study"); this.resume_at = this.elapsed + 1.5;
        this.phase = 85; return;
    }
    if this.phase == 85 {
        must_cmd("OpenTwin", #{path: ctx.bad_twin}); this.phase = 9; return;
    }
    if this.phase == 9 {
        if state.twin_id == this.good_id || state.error == () { return; }
        this.bad_id = state.twin_id;
        this.checks.push(t_true(!contains_id(state.perspectives, "twin/power-study") && !contains_id(state.perspectives, "twin/invalid"), "replacement retires outgoing names and invalid plans install no custom layout"));
        this.checks.push(t_true(state.error.contains("missing-panel") && contains_id(state.perspectives, "analysis"), "missing capabilities fail visibly while application tasks remain usable"));
        must_cmd("ActivatePerspective", #{id: "analysis"}); this.phase = 10; return;
    }
    if this.phase == 10 {
        if state.active_perspective != "analysis" { return; }
        this.checks.push(t_true(state.docked_panels.contains("modelica_plot"), "a rejected custom layout does not crash or disable analysis"));
        must_cmd("OpenTwin", #{path: ctx.good_twin}); this.phase = 11; return;
    }
    if this.phase == 11 {
        if state.twin_id == this.bad_id || !contains_id(state.perspectives, "twin/power-study") { return; }
        this.checks.push(t_true(state.error == () && state.twin_id != this.good_id, "A to B to A receives a fresh Twin lease and fresh policy definitions"));
        must_cmd("ActivatePerspective", #{id: "twin/power-study"}); this.phase = 12; return;
    }
    if state.active_perspective != "twin/power-study" { return; }
    this.checks.push(t_true(!state.docked_panels.contains("sandbox_joint_state"), "reopening a Twin does not revive a retired task's transient tools"));
    this.done = true; finish(this.checks);
}
