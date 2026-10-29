// Generic menu action for file-backed authored tutorial scenarios.
fn start(args) {
    cmd("ClearGuidedOverlay", #{});
    if args.twin_asset != () {
        return cmd("OpenTwinAsset", #{manifest_asset: args.twin_asset});
    }
    let request = #{
        source_asset: args.source_asset,
        scene_asset: args.scene_asset,
        reload_policy: args.reload_policy,
        params: if args.params == () { #{} } else { args.params },
    };
    if args.owner_twin_id != () {
        request.owner_twin_id = args.owner_twin_id;
    }
    cmd("RunScenarioAsset", request)
}

/// Deliver a leased HUD action to its authored lesson. The lesson validates
/// its own action id and current objective before applying any domain command.
fn action(id) { emit("tutorial.action", id); }

/// Show an actual continuation from the catalog's resolved learning path.
/// Standalone scene tests have no catalog path and receive no promised button.
fn finish(ctx, message) {
    hint(message);
    if ctx.lesson_path == () || ctx.lesson_path.len() == 0 { return; }
    let current = ctx.lesson_path[0];
    let actions = [];
    if ctx.lesson_path.len() > 1 {
        actions.push(#{ id: current.source_asset + ":next",
            label: "Next: " + ctx.lesson_path[1].title, enabled: true });
    }
    actions.push(#{ id: current.source_asset + ":repeat",
        label: "Restart lesson", enabled: true });
    guided_hud_actions("tutorials", "action", actions);
}

/// Consume only this catalog lesson's exact navigation action. The remaining
/// path retains the next lesson's metadata and continuation without a registry.
fn navigation(evt, ctx) {
    if evt.name != "tutorial.action" || ctx.lesson_path == ()
        || ctx.lesson_path.len() == 0 { return false; }
    let current = ctx.lesson_path[0];
    if evt.value == current.source_asset + ":repeat" {
        cmd("RestartScene", #{});
        return true;
    }
    if evt.value != current.source_asset + ":next" { return false; }
    if ctx.lesson_path.len() < 2 { throw "this lesson has no catalog continuation"; }
    let path = ctx.lesson_path;
    path.remove(0);
    start(#{ source_asset: path[0].source_asset, scene_asset: path[0].scene_asset, twin_asset: path[0].twin_asset,
        reload_policy: "Restart", params: #{ lesson_path: path } });
    true
}

/// Advance one authored tour cursor. A tour reviews guidance; only exercises
/// assess domain state. Closed/skipped tours cannot consume later Next events.
fn tour_event(state, evt, steps, ctx) {
    if state.tour_closed == true { return state; }
    if evt.name == "cmd:GuidedSkip" {
        state.tour_closed = true;
        end_tour();
        hint("Tour skipped. Choose an exercise from Tutorials when ready.");
        emit("TUTORIAL_SKIPPED", 0);
        return state;
    }
    if evt.name == "cmd:GuidedBack" {
        if state.i > 0 { state.i -= 1; }
    } else if evt.name == "cmd:GuidedGoto" {
        if evt.value < 0 || evt.value >= steps.len() { throw "tutorial step is outside this tour"; }
        state.i = evt.value;
    } else {
        let required = steps[state.i].require;
        let matched = if required == () { evt.name == "cmd:GuidedNext" }
            else if string_is(required) { evt.name == required }
            else { required.contains(evt.name) };
        if !matched { return state; }
        state.i += 1;
    }
    if state.i >= steps.len() {
        state.tour_closed = true;
        end_tour();
        finish(ctx, "Tour reviewed. The actions shown here were not assessed. Choose an exercise to practise.");
        emit("MISSION_COMPLETE", 0);
    } else { coach_step(steps, state.i); }
    state
}
