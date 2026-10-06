// TEST-ONLY: framing uses current view identities without changing topology.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = []; }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 20.0 { this.checks.push(t_true(false, "framing timeout")); this.done = true; report_verdict(this.checks, "DIAGRAM FOCUS", "DIAGRAM_FOCUS"); return; }
    let diagram = query("InspectConnectionDiagram");
    if diagram == () || !diagram.built { return; }
    if this.phase == 0 {
        this.view = diagram.view_id; this.scope = diagram.scope; this.links = diagram.connection_count;
        this.result = cmd("FrameConnectionDiagram", #{ view_id: this.view, key: ctx.node });
        this.phase = 1; return;
    }
    let result = command_result(this.result.id);
    if result.status == "pending" { return; }
    if this.phase == 1 {
        if result.status != "applied" { this.checks.push(t_true(false, "focus command rejected")); this.done = true; report_verdict(this.checks, "DIAGRAM FOCUS", "DIAGRAM_FOCUS"); return; }
        let node = (); for candidate in diagram.nodes { if candidate.key == ctx.node { node = candidate; } }
        if node == () { return; }
        if diagram.viewport.center_x != node.x + node.width / 2.0 || diagram.viewport.center_y != node.y + node.height / 2.0 { return; }
        this.checks.push(t_true(diagram.viewport.zoom > 0.0 && diagram.viewport.zoom <= 1.0, "focus centers the exact card at readable natural scale"));
        this.checks.push(t_true(diagram.scope == this.scope && diagram.connection_count == this.links, "framing preserves USD topology and scope"));
        this.focus_zoom = diagram.viewport.zoom;
        this.result = cmd("FrameConnectionDiagram", #{ view_id: this.view }); this.phase = 2; return;
    }
    if this.phase == 2 {
        if diagram.viewport.zoom >= this.focus_zoom { return; }
        this.checks.push(t_true(result.status == "applied", "fit restores the complete system overview"));
        this.before = diagram.viewport;
        this.result = cmd("FrameConnectionDiagram", #{ view_id: this.view, key: "/AbsentCard" }); this.phase = 3; return;
    }
    this.checks.push(t_true((result.status == "failed" || result.status == "rejected") && diagram.viewport == this.before, "missing card rejects without moving the viewport"));
    this.done = true; report_verdict(this.checks, "DIAGRAM FOCUS", "DIAGRAM_FOCUS");
}
