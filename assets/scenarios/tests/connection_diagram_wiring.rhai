// TEST-ONLY: exercise composed port authoring in an owned, unsaved document.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = []; }
fn wire(ctx, sink, sources) {
    cmd("ApplyUsdOps", #{ doc_id: ctx.doc_id, label: "Test composed connection", ops: [
        #{ SetConnection: #{ edit_target: "@root@", path: sink, name: ctx.sink_port,
            type_name: ctx.port_type, sources: sources } }
    ] })
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.checks.push(t_true(false, "composed wiring timeout phase " + this.phase));
        this.done = true; report_verdict(this.checks, "DIAGRAM WIRING", "DIAGRAM_WIRING"); return;
    }
    let diagram = query("InspectConnectionDiagram");
    if diagram == () || !diagram.built || diagram.active_scene { return; }
    if this.phase == 0 {
        this.before = diagram.connection_count.to_int();
        let sources = [];
        for edge in diagram.connections {
            if edge.target == ctx.sink_path && edge.target_port == ctx.sink_port {
                sources.push(edge.source + "." + edge.source_port);
            }
        }
        sources.push(ctx.source_path + "." + ctx.source_port);
        this.result = wire(ctx, ctx.sink_path, sources); this.phase = 1; return;
    }
    let result = command_result(this.result.id);
    if result.status == "pending" && this.phase != 2 { return; }
    if this.phase == 1 {
        let found = false;
        for edge in diagram.connections {
            if edge.source == ctx.source_path && edge.source_port == ctx.source_port && edge.target == ctx.sink_path && edge.target_port == ctx.sink_port { found = true; }
        }
        if result.status == "applied" && !found { return; }
        this.checks.push(t_true(result.status == "applied", "referenced sink accepts a local connection opinion"));
        this.checks.push(t_true(found && diagram.connection_count.to_int() == this.before + 1, "composed wiring appears in the USD diagram"));
        if result.status != "applied" {
            this.done = true; report_verdict(this.checks, "DIAGRAM WIRING", "DIAGRAM_WIRING"); return;
        }
        this.result = cmd("UndoDocument", #{ doc_id: ctx.doc_id }); this.phase = 2; return;
    }
    if this.phase == 2 {
        if diagram.connection_count.to_int() != this.before { return; }
        this.checks.push(t_true(diagram.connection_count.to_int() == this.before, "undo removes the local opinion and restores inherited topology"));
        this.result = wire(ctx, ctx.sink_path + "/AbsentPortOwner", [ctx.source_path + "." + ctx.source_port]); this.phase = 3; return;
    }
    this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "missing composed sink rejects without crashing"));
    this.checks.push(t_true(diagram.connection_count.to_int() == this.before, "rejected wiring leaves topology intact"));
    this.done = true; report_verdict(this.checks, "DIAGRAM WIRING", "DIAGRAM_WIRING");
}
