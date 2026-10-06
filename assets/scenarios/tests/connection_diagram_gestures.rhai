// TEST-ONLY: native mouse gestures through the production canvas and USD journal.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.wait = 0; this.done = false; this.checks = []; }
fn point(d, key, port) {
    for node in d.nodes { if node.key == key { for p in node.ports { if p.name == port { return #{x:p.screen_x, y:p.screen_y}; } } } }
    throw "Requested gesture endpoint is absent";
}
fn pointer(p) { cmd("InjectWindowInput", #{event: #{PointerMove: p}}); }
fn button(p, state) { cmd("InjectWindowInput", #{event: #{PointerButton: #{button:"Primary", state:state, x:p.x, y:p.y}}}); }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 25.0 { this.checks.push(t_true(false, "gesture timeout phase " + this.phase)); this.done = true; report_verdict(this.checks, "DIAGRAM GESTURES", "DIAGRAM_GESTURES"); return; }
    let d = query("InspectConnectionDiagram");
    if d == () || d.built != true || d.active_scene { return; }
    if this.phase == 0 {
        this.before = d.connection_count.to_int();
        this.result = cmd("FrameConnectionDiagram", #{view_id:d.view_id, key:ctx.sink}); this.phase = 1; return;
    }
    if this.phase == 1 {
        let node = (); for n in d.nodes { if n.key == ctx.sink { node = n; } }
        if node == () || d.viewport.center_x != node.x + node.width / 2.0 { return; }
        this.viewport = d.viewport;
        this.sink_x = node.x; this.sink_y = node.y;
        this.from = point(d, ctx.source, ctx.source_port); this.to = point(d, ctx.sink, ctx.sink_port);
        pointer(this.from); this.phase = 2; return;
    }
    // Give native input its own rendered frames; commands are admitted before paint.
    this.wait += 1; if this.wait < 4 { return; } this.wait = 0;
    if this.phase == 2 { button(this.from, "Pressed"); this.phase = 3; return; }
    if this.phase == 3 { pointer(this.to); this.phase = 4; return; }
    if this.phase == 4 { button(this.to, "Released"); this.phase = 5; return; }
    if this.phase == 5 {
        let found = false;
        for edge in d.connections { if edge.source_key == ctx.source && edge.source_port == ctx.source_port && edge.target_key == ctx.sink && edge.target_port == ctx.sink_port { found = true; } }
        if !found { return; }
        this.checks.push(t_true(d.connection_count.to_int() == this.before + 1, "port drag authors exactly one USD connection"));
        this.checks.push(t_true(d.viewport == this.viewport, "authoring preserves the exploration viewport"));
        this.result = cmd("UndoDocument", #{doc_id:ctx.doc_id}); this.phase = 6; return;
    }
    if this.phase == 6 {
        if d.connection_count.to_int() != this.before { return; }
        let restored = false;
        for node in d.nodes { if node.key == ctx.sink && node.x == this.sink_x && node.y == this.sink_y { restored = true; } }
        if !restored { return; }
        this.checks.push(t_true(true, "Undo restores inherited topology after a mouse gesture"));
        this.from = point(d, ctx.sink, ctx.invalid_port); this.to = point(d, ctx.sink, ctx.sink_port);
        pointer(this.from); this.phase = 7; return;
    }
    if this.phase == 7 { button(this.from, "Pressed"); this.phase = 8; return; }
    if this.phase == 8 { pointer(this.to); this.phase = 9; return; }
    if this.phase == 9 { button(this.to, "Released"); this.phase = 10; return; }
    if d.error == () { return; }
    this.checks.push(t_true(d.error.contains("cannot connect"), "invalid drag explains the rejected port direction"));
    this.checks.push(t_true(d.connection_count.to_int() == this.before, "invalid drag preserves USD topology"));
    this.done = true; report_verdict(this.checks, "DIAGRAM GESTURES", "DIAGRAM_GESTURES");
}
