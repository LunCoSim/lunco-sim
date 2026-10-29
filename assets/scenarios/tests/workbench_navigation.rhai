// @timing presentation
// Source navigation and Twin retirement through the production behavior tree.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectWorkbench", "InspectWorkbenchNavigation"], required_inputs: []}
}
fn nav() { query("InspectWorkbenchNavigation") }
fn record(state, condition, label) {
    state.checks.push(t_true(condition, label));
    if !condition { state.failure = label; }
    state
}
fn finish(state, failure) {
    if state.done { return state; }
    if failure != "" { state.checks.push(t_true(false, failure)); }
    state.done = true;
    report_verdict(state.checks, "Workbench navigation", "WORKBENCH_NAVIGATION");
    state
}
fn valid(me) { this.failure == "" }
fn ready(me) {
    let state = query("InspectWorkbench"); let current = nav();
    if state == () || current == () || state.preparing || !state.scene_mounted || current.twin_id == () { return false; }
    for perspective in state.perspectives { if perspective.id == "editor" { return true; } }
    false
}
fn begin(me) {
    let current = nav(); this.scene = current.scene_path; this.twin = current.twin_id;
    this = record(this, current.twin_root != () && this.scene != "", "Twin and loaded scene have independent exact context");
    cmd("ActivatePerspective", #{id: "editor"}); cmd("SearchNavigation", #{});
}
fn twin_results_ready(me) { let current = nav(); current.open && current.entries.len() > 0 }
fn open_twin_source(me) {
    let only_twin = true; let target = ();
    for entry in nav().entries {
        if entry.category != "Twin" { only_twin = false; }
        if entry.label == "site.usda" { target = entry.id; }
    }
    this = record(this, only_twin, "empty quick-open prioritizes only active-Twin files");
    this = record(this, target != (), "quick-open finds the source in the existing Twin inventory");
    if target != () { cmd("ActivateNavigationEntry", #{id: target}); }
}
fn preview_ready(me) { let current = nav(); current.source != () && current.mode != () && current.mode.contains("USD preview") }
fn inspect_preview(me) {
    let current = nav(); this.source = current.source;
    this = record(this, this.source.ends_with("site.usda"), "USD sources identify their isolated editing preview");
    this = record(this, current.scene_path == this.scene && current.twin_id == this.twin, "opening a source preserves the loaded scene and active Twin");
    print("NAV_CAPTURE:preview");
}
fn find_library(me) {
    cmd("RevealNavigationSource", #{uri: this.source});
    cmd("SearchNavigation", #{query: "workbench_perspectives", category: "Libraries"});
}
fn library_results_ready(me) { let current = nav(); current.query == "workbench_perspectives" && current.entries.len() > 0 }
fn inspect_library_results(me) {
    this.library_target = ();
    for entry in nav().entries {
        if entry.detail.contains("scripting/policy/workbench_perspectives.rhai") { this.library_target = entry.id; }
    }
    this = record(this, this.library_target != (), "library search shows readable source paths and explicit scope");
    print("NAV_CAPTURE:library-search");
}
fn open_library(me) { cmd("ActivateNavigationEntry", #{id: this.library_target}); }
fn library_ready(me) { nav().mode == "Read-only source" }
fn inspect_library(me) {
    let current = nav();
    this = record(this, current.source.contains("workbench_perspectives.rhai"), "library navigation opens the exact read-only source");
    this = record(this, current.scene_path == this.scene, "library inspection preserves the loaded world");
    this = record(this, current.can_back, "document visits create back navigation");
    cmd("NavigateHistory", #{forward: false});
}
fn back_ready(me) { nav().source == this.source }
fn go_forward(me) {
    this = record(this, nav().can_forward, "Back restores the prior source and retains a forward destination");
    cmd("NavigateHistory", #{forward: true});
}
fn reject_destination(me) {
    this = record(this, nav().scene_path == this.scene, "Forward restores a source without replaying scene loads");
    cmd("ActivateNavigationEntry", #{id: "missing:navigation-destination"});
}
fn rejection_ready(me) { nav().error != () }
fn empty_search(me) {
    let current = nav();
    this = record(this, current.error.contains("no longer available") && current.scene_path == this.scene, "an unavailable destination rejects visibly without changing the world");
    cmd("SearchNavigation", #{query: "no-such-navigation-file-7492", category: "Twin"});
}
fn empty_results_ready(me) { nav().query == "no-such-navigation-file-7492" }
fn find_scenes(me) {
    let current = nav();
    this = record(this, current.open && current.entries.len() == 0, "empty search results remain explicit");
    cmd("SearchNavigation", #{category: "Scenes"});
}
fn scenes_ready(me) { let current = nav(); current.category == "Scenes" && current.entries.len() > 0 }
fn inspect_scenes(me) {
    let explicit_loads = true;
    for entry in nav().entries { if !entry.label.starts_with("Load ") || !entry.detail.contains("replaces the loaded scene") { explicit_loads = false; } }
    this = record(this, explicit_loads, "scene choices come from indexed inventories and label world replacement explicitly");
}
fn press_escape(me) { input_key_press("Escape"); }
fn release_escape(me) { input_key_release("Escape"); }
fn search_closed(me) { !nav().open }
fn press_control(me) { input_key_press("ControlLeft"); }
fn press_control_shift(me) { input_key_press("ControlLeft"); input_key_press("ShiftLeft"); }
fn press_p(me) { input_key_press("KeyP"); }
fn release_shortcut(me) { input_key_release("KeyP"); input_key_release("ShiftLeft"); input_key_release("ControlLeft"); }
fn quick_open_ready(me) { let current = nav(); current.open && current.category == "" }
fn inspect_quick_open(me) {
    this = record(this, nav().query == "", "native application Ctrl+P opens quick navigation");
    print("NAV_CAPTURE:quick-open");
}
fn press_enter(me) { input_key_press("Enter"); }
fn release_enter(me) { input_key_release("Enter"); }
fn entered_source_ready(me) { let current = nav(); !current.open && current.source == this.source }
fn inspect_entered_source(me) {
    let current = nav();
    this = record(this, current.mode.contains("USD preview") && current.scene_path == this.scene, "native Enter activates the selected source without loading a world");
}
fn actions_ready(me) { let current = nav(); current.open && current.category == "Actions" }
fn inspect_actions(me) {
    let current = nav(); let actions_only = current.entries.len() > 0;
    for entry in current.entries { if entry.category != "Actions" { actions_only = false; } }
    this = record(this, actions_only, "native application Ctrl+Shift+P opens panel and perspective actions");
    print("NAV_CAPTURE:actions");
}
fn replace_twin(me) { cmd("OpenTwin", #{path: this.replacement}); }
fn replacement_ready(me) { let current = nav(); current.twin_id != () && current.twin_id != this.twin }
fn inspect_retirement(me) {
    let current = nav();
    this = record(this, !current.open && current.query == "" && !current.can_back && current.source == (), "Twin replacement retires search, source context, and navigation history");
    this = finish(this, "");
}
fn timeout(me) { this = finish(this, "navigation behavior tree timed out"); }
fn on_start(me, ctx) {
    this.done = false; this.failure = ""; this.checks = [];
    this.replacement = ctx.replacement; subscribe("TASK_FAILED");
}
fn on_event(me, event, ctx) {
    if event.name == "TASK_FAILED" { this = finish(this, "navigation task failed: " + this.failure); }
}
fn on_stop(me, ctx) { if !this.done { this = finish(this, "navigation stopped before its behavior tree produced a verdict"); } }
fn task(me, ctx) {
    par_race([
        seq([
            wait_until(Fn("ready")), once(Fn("begin")), check(Fn("valid")),
            wait_until(Fn("twin_results_ready")), once(Fn("open_twin_source")), check(Fn("valid")),
            wait_until(Fn("preview_ready")), once(Fn("inspect_preview")), check(Fn("valid")), wait(1.0),
            once(Fn("find_library")), wait_until(Fn("library_results_ready")), once(Fn("inspect_library_results")), check(Fn("valid")), wait(1.0),
            once(Fn("open_library")), wait_until(Fn("library_ready")), once(Fn("inspect_library")), check(Fn("valid")),
            wait_until(Fn("back_ready")), once(Fn("go_forward")), check(Fn("valid")), wait_until(Fn("library_ready")),
            once(Fn("reject_destination")), wait_until(Fn("rejection_ready")), once(Fn("empty_search")), check(Fn("valid")),
            wait_until(Fn("empty_results_ready")), once(Fn("find_scenes")), check(Fn("valid")),
            wait_until(Fn("scenes_ready")), once(Fn("inspect_scenes")), check(Fn("valid")),
            once(Fn("press_escape")), wait(0.05), once(Fn("release_escape")), wait_until(Fn("search_closed")),
            once(Fn("press_control")), wait(0.05), once(Fn("press_p")), wait(0.05), once(Fn("release_shortcut")),
            wait_until(Fn("quick_open_ready")), once(Fn("inspect_quick_open")), check(Fn("valid")), wait(1.0),
            once(Fn("press_enter")), wait(0.05), once(Fn("release_enter")), wait_until(Fn("entered_source_ready")),
            once(Fn("inspect_entered_source")), check(Fn("valid")),
            once(Fn("press_control_shift")), wait(0.05), once(Fn("press_p")), wait(0.05), once(Fn("release_shortcut")),
            wait_until(Fn("actions_ready")), once(Fn("inspect_actions")), check(Fn("valid")), wait(1.0),
            once(Fn("replace_twin")), wait_until(Fn("replacement_ready")), once(Fn("inspect_retirement")),
        ]),
        seq([wait(45.0), once(Fn("timeout"))]),
    ])
}
