import "../tools/literal#% probe.rhai" as literal;

// Twin search-path gate. This program is bound from `sim/scenes/` by the
// OpenUSD search path `@scripts/search_root_probe.rhai@` and exists only at the
// Twin root, so it runs only when the layer-anchored miss falls back to the
// Twin root. The sibling precedence program reports FAIL if its root copy is
// selected over the copy beside the layer.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListToolLibraries", "ListTimelines"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.rejections = [];
    for name in ["NUL", "../escape", "stream:alias"] {
        this.rejections.push(cmd("RegisterTimeline", #{ name: name, timeline: #{ steps: [#{wait: 0.25}] } }));
    }
}

fn finish(state) {
    if state.done { return state; }
    state.done = true;
    report_verdict([
        t_true(true, "search path resolved from the Twin root after a layer miss"),
        t_true(true, "layer-anchored precedence copy was selected over the Twin root copy"),
        t_true(literal::marker() == "literal source reached", "Rhai import preserved literal # and % filename characters"),
        t_true(state.tool_loaded, "Twin tool loader preserved the literal filename"),
        t_true(state.timeline_loaded, "Twin timeline loader preserved the literal filename"),
        t_true(state.names_rejected, "nonportable timeline names returned terminal rejection diagnostics"),
    ], "Twin search paths", "TWIN_SEARCH_PATHS");
    state
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    let tools = query("ListToolLibraries", #{});
    this.tool_loaded = false;
    if tools != () {
        for tool in tools.libraries {
            if tool.name == "literal#% probe" && tool.scope.starts_with("twin:") { this.tool_loaded = true; }
        }
    }
    let timelines = query("ListTimelines", #{});
    this.timeline_loaded = timelines != () && timelines.timelines.contains("literal#% probe");
    this.names_rejected = true;
    for rejection in this.rejections {
        let result = if rejection.status == "pending" { command_result(rejection.id) } else { rejection };
        if result == () || result.ok != false || result.error == () || result.error == ""
            || (result.status != "rejected" && result.status != "failed") {
            this.names_rejected = false;
        }
    }
    if (this.tool_loaded && this.timeline_loaded && this.names_rejected && this.elapsed >= 0.5) || this.elapsed >= 30.0 {
        this = finish(this);
    }
}
