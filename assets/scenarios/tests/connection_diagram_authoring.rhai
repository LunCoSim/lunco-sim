// TEST-ONLY: run in an owned Editor document preview; never save authored USD.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = []; }
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.checks.push(t_true(false, "diagram authoring timeout phase " + this.phase));
        this.done = true; report_verdict(this.checks, "DIAGRAM AUTHORING", "DIAGRAM_AUTHORING"); return;
    }
    let diagram = query("InspectConnectionDiagram");
    if diagram == () || !diagram.built || diagram.active_scene { return; }
    if this.phase == 0 {
        this.checks.push(t_true(diagram.connection_count.to_string() == ctx.expected_connections.to_string(), "authored feedback remains in the diagram"));
        let keys = #{}; let unique = true;
        for node in diagram.nodes { if keys.contains(node.key) { unique = false; } keys[node.key] = true; }
        this.checks.push(t_true(unique, "presentation identities are distinct"));
        this.view = diagram.view_id;
        this.result = cmd("DropConnectionAsset", #{ view_id: this.view, source: ctx.reference_asset, x: 120.0, y: 140.0 });
        this.phase = 1; return;
    }
    let result = command_result(this.result.id);
    if result.status == "pending" { return; }
    if this.phase == 1 {
        let found = false;
        for node in diagram.nodes { if node.path == ctx.expected_reference { found = node.x == 120.0 && node.y == 140.0; } }
        if result.status == "applied" && !found { return; }
        this.checks.push(t_true(result.status == "applied", "USD reference reaches terminal journal success: " + result));
        this.checks.push(t_true(found, "referenced USD prim is placed in its named view"));
        this.result = cmd("DropConnectionAsset", #{ view_id: this.view, source: ctx.program_asset, target_path: ctx.host_path, x: 200.0, y: 220.0 });
        this.phase = 2; return;
    }
    if this.phase == 2 {
        let found = false;
        for node in diagram.nodes { for program in node.programs { if program.source == ctx.program_asset { found = program.backend == ctx.backend && program.issue == (); } } }
        if result.status == "applied" && !found { return; }
        this.checks.push(t_true(result.status == "applied", "program reaches terminal journal success: " + result));
        this.checks.push(t_true(found, "program facet belongs to the composed USD prim"));
        this.result = cmd("DropConnectionAsset", #{ view_id: this.view, source: ctx.program_asset, target_path: "/AbsentDropTarget", x: 0.0, y: 0.0 });
        this.phase = 3; return;
    }
    if this.phase == 3 {
        this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "absent drop target rejects without crashing"));
        this.result = cmd("DropConnectionAsset", #{ view_id: this.view, source: "notes.txt", x: 0.0, y: 0.0 });
        this.phase = 4; return;
    }
    this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "unsupported drop rejects without changing USD"));
    this.done = true; report_verdict(this.checks, "DIAGRAM AUTHORING", "DIAGRAM_AUTHORING");
}
