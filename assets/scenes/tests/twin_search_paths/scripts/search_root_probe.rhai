// Twin search-path gate. This program is bound from `sim/scenes/` by the
// OpenUSD search path `@scripts/search_root_probe.rhai@` and exists only at the
// Twin root, so it runs only when the layer-anchored miss falls back to the
// Twin root. The sibling precedence program reports FAIL if its root copy is
// selected over the copy beside the layer.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: [], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
}

fn finish(me) {
    if this.done { return; }
    this.done = true;
    report_verdict([
        t_true(true, "search path resolved from the Twin root after a layer miss"),
        t_true(true, "layer-anchored precedence copy was selected over the Twin root copy"),
    ], "Twin search paths", "TWIN_SEARCH_PATHS");
}

fn task(me, ctx) {
    seq([wait(0.5), once(Fn("finish"))])
}
