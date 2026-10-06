// TEST-ONLY: selected endpoints, navigation recovery and rejected authoring.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = []; }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 25.0 { this.checks.push(t_true(false, "exploration timeout")); this.done = true; report_verdict(this.checks, "DIAGRAM EXPLORE", "DIAGRAM_EXPLORE"); return; }
    let d = query("InspectConnectionDiagram");
    if d == () { return; }
    if d.built != true { this.checks.push(t_true(false, "diagram query unavailable: " + d.error)); this.done = true; report_verdict(this.checks, "DIAGRAM EXPLORE", "DIAGRAM_EXPLORE"); return; }
    if this.phase == 0 {
        this.view = d.view_id; this.scope = d.scope; this.links = d.connection_count;
        this.result = cmd("SelectConnectionElement", #{ view_id: this.view, key: ctx.node, port: ctx.port });
        this.phase = 1; return;
    }
    let result = command_result(this.result.id); if result.status == "pending" { return; }
    if this.phase == 1 {
        if result.status == "applied" && (d.selection.key != ctx.node || d.selection.port != ctx.port) { return; }
        this.checks.push(t_true(result.status == "applied" && d.selection.key == ctx.node && d.selection.port == ctx.port, "exact USD port is selected"));
        this.checks.push(t_true(d.selection.connection_count.to_int() > 0 && d.selection.connection_count.to_int() < d.connection_count.to_int(), "port trace excludes unrelated connections"));
        this.result = cmd("FrameConnectionDiagram", #{ view_id: this.view, key: ctx.node }); this.phase = 2; return;
    }
    if this.phase == 2 {
        let node = (); for n in d.nodes { if n.key == ctx.node { node = n; } }
        if node == () || d.viewport.center_x != node.x + node.width / 2.0 { return; }
        this.before = d.viewport;
        this.result = cmd("NavigateConnectionDiagram", #{ view_id: this.view, scope: ctx.child }); this.phase = 3; return;
    }
    if this.phase == 3 {
        if result.status == "applied" && d.scope != ctx.child { return; }
        this.checks.push(t_true(result.status == "applied" && d.scope == ctx.child, "explicit system navigation uses composed USD scope"));
        this.result = cmd("NavigateConnectionDiagram", #{ view_id: this.view, back: true }); this.phase = 4; return;
    }
    if this.phase == 4 {
        if d.scope != this.scope || d.viewport != this.before { return; }
        this.checks.push(t_true(true, "Back restores previous viewport"));
        this.result = cmd("SelectConnectionElement", #{ view_id: this.view, key: ctx.node, port: "inputs:missing" }); this.phase = 5; return;
    }
    if this.phase == 5 {
        this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "missing port rejects without fabricating an interface"));
        this.result = cmd("NavigateConnectionDiagram", #{ view_id: this.view, scope: "/AbsentSystem" }); this.phase = 6; return;
    }
    this.checks.push(t_true((result.status == "failed" || result.status == "rejected") && d.scope == this.scope && d.connection_count == this.links, "invalid navigation preserves topology and scope"));
    this.done = true; report_verdict(this.checks, "DIAGRAM EXPLORE", "DIAGRAM_EXPLORE");
}
