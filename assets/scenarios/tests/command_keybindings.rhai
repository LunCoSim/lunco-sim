// @timing presentation
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectInputBindings", "InspectWorkbenchNavigation"], required_inputs: []}
}
fn bindings() { query("InspectInputBindings") }
fn nav() { query("InspectWorkbenchNavigation") }
fn intent_keys(entries, name) { for entry in entries { if entry.name == name { return entry.keys; } } [] }
fn binding(entries, id) { for entry in entries { if entry.id == id { return entry; } } () }
fn record(state, condition, label) { state.checks.push(t_true(condition, label)); if !condition { state.failure = label; } state }
fn finish(state, failure) {
    if state.done { return state; }
    if failure != "" { state.checks.push(t_true(false, failure)); }
    state.done = true;
    report_verdict(state.checks, "Command keybindings", "COMMAND_KEYBINDINGS"); state
}
fn valid(me) { this.failure == "" }
fn ready(me) { binding(bindings().global_commands, "editor.save") != () }
fn install_global(me) {
    cmd("SetInputIntentBinding", #{intent: "forward", keys: ["KeyW"]});
    cmd("SetCommandKeyBinding", #{id: "test.global", keys: ["KeyK"], command: "SearchNavigation", arguments: [#{name: "query", value: #{Text: "Global shortcut"}}], allow_in_text: true});
}
fn global_ready(me) { binding(bindings().global_commands, "test.global") != () }
fn inspect_global(me) {
    let state = bindings();
    this = record(this, binding(state.global_commands, "test.global").keys.contains("KeyK"), "a global command shortcut is stored in the shared input settings");
    this = record(this, binding(state.global_commands, "editor.save").command == "InvokeEditorIntent", "document shortcuts share the configurable command owner");
    this.rejection = cmd("SetCommandKeyBinding", #{id: "test.invalid", keys: ["KeyJ"], command: "MissingKeybindingCommand7492"});
}
fn rejection_ready(me) {
    if this.rejection.status == "pending" { this.rejection = command_result(this.rejection.id); }
    this.rejection.status != "pending"
}
fn reject_unknown(me) {
    this = record(this, !this.rejection.ok && binding(bindings().global_commands, "test.invalid") == (), "an unknown command rejects without altering settings");
    cmd("OpenTwin", #{path: this.override_twin});
}
fn overrides_ready(me) {
    let state = bindings();
    let twin = binding(state.effective_commands, "test.twin");
    twin != () && twin.keys.contains("KeyK") && state.collisions.len() >= 2 && state.recent_events.len() >= 2
}
fn inspect_overrides(me) {
    let state = bindings();
    this = record(this, intent_keys(state.global_intents, "forward").contains("KeyW") && binding(state.global_commands, "test.global").keys.contains("KeyK"), "Twin composition preserves the stored global settings");
    this = record(this, !intent_keys(state.effective_intents, "forward").contains("KeyW") && intent_keys(state.effective_intents, "action").contains("KeyW"), "a Twin action takes physical W ownership from the global forward action");
    this = record(this, binding(state.effective_commands, "test.global").keys.len() == 0 && binding(state.effective_commands, "test.twin").keys.contains("KeyK"), "a Twin command displaces the global command on the same chord");
    let w = false; let k = false;
    for notice in state.recent_events {
        if notice.contains("intent:forward") && notice.contains("intent:action") { w = true; }
        if notice.contains("command:test.global") && notice.contains("command:test.twin") { k = true; }
    }
    this = record(this, w && k, "both collisions identify the winner and displaced action in Recent Events");
    this.notice_count = state.recent_events.len();
    cmd("FocusPanel", #{id: "input_keybindings"});
}
fn capture_settings(me) { print("BINDINGS_CAPTURE:settings"); }
fn point_at_settings(me) { input_pointer_move(600.0, 400.0); }
fn scroll_settings(me) { input_scroll(0.0, -100.0); }
fn capture_settings_form(me) { print("BINDINGS_CAPTURE:settings-form"); }
fn inspect_no_repeat(me) {
    this = record(this, bindings().recent_events.len() == this.notice_count, "unchanged bindings do not repeat collision notifications");
}
fn press_k(me) { input_key_press("KeyK"); }
fn release_k(me) { input_key_release("KeyK"); }
fn twin_shortcut_ready(me) { nav().open && nav().query == "Twin shortcut" }
fn inspect_twin_shortcut(me) {
    this = record(this, nav().query == "Twin shortcut", "native K executes only the winning Twin command and its typed parameter");
    cmd("OpenTwin", #{path: this.replacement});
}
fn restored(me) {
    let state = bindings();
    binding(state.effective_commands, "test.twin") == () && state.collisions.len() == 0 && intent_keys(state.effective_intents, "forward").contains("KeyW")
}
fn inspect_restored(me) {
    this = record(this, binding(bindings().effective_commands, "test.global").keys.contains("KeyK"), "closing the Twin restores the global command and semantic binding");
}
fn global_shortcut_ready(me) { nav().open && nav().query == "Global shortcut" }
fn complete(me) {
    this = record(this, nav().query == "Global shortcut", "native K executes the restored global command after Twin replacement");
    this = finish(this, "");
}
fn timeout(me) { this = finish(this, "command keybinding behavior tree timed out"); }
fn on_start(me, ctx) {
    this.done = false; this.failure = ""; this.checks = [];
    this.override_twin = ctx.override_twin; this.replacement = ctx.replacement; subscribe("TASK_FAILED");
}
fn on_event(me, event, ctx) { if event.name == "TASK_FAILED" { this = finish(this, "keybinding task failed: " + this.failure); } }
fn on_stop(me, ctx) { if !this.done { this = finish(this, "keybinding behavior tree stopped before its verdict"); } }
fn task(me, ctx) {
    par_race([
        seq([
            wait_until(Fn("ready")), once(Fn("install_global")), wait_until(Fn("global_ready")), once(Fn("inspect_global")), check(Fn("valid")),
            wait_until(Fn("rejection_ready")), once(Fn("reject_unknown")), check(Fn("valid")),
            wait_until(Fn("overrides_ready")), once(Fn("inspect_overrides")), check(Fn("valid")), wait(0.5), once(Fn("capture_settings")), wait(1.0), once(Fn("point_at_settings")), wait(0.05), once(Fn("scroll_settings")), wait(0.5), once(Fn("capture_settings_form")), wait(1.0),
            once(Fn("inspect_no_repeat")), check(Fn("valid")),
            once(Fn("press_k")), wait(0.05), once(Fn("release_k")), wait_until(Fn("twin_shortcut_ready")),
            once(Fn("inspect_twin_shortcut")), check(Fn("valid")), wait_until(Fn("restored")), once(Fn("inspect_restored")), check(Fn("valid")),
            once(Fn("press_k")), wait(0.05), once(Fn("release_k")), wait_until(Fn("global_shortcut_ready")), once(Fn("complete")),
        ]),
        seq([wait(45.0), once(Fn("timeout"))]),
    ])
}
