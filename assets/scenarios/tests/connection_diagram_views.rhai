// TEST-ONLY: named views are presentation documents over the loaded USD.
const TEST_KIND = "editor";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["InspectConnectionDiagram"], required_inputs: [] }
}
fn on_start(me, ctx) {
    this.expect_runtime_ports = ctx.expect_runtime_ports == true;
    this.open_request = (); this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = [];
    cmd("ActivatePerspective", #{ id: "editor" });
    cmd("FocusPanel", #{ id: "usd_connection_canvas" });
}
fn definition(diagram, name) {
    for view in diagram.definitions { if view.name == name { return view; } }
    ()
}
fn finish(checks) {
    report_verdict(checks, "CONNECTION DIAGRAM VIEWS", "CONNECTION_DIAGRAM_VIEWS");
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.checks.push(t_true(false, "diagram views timeout phase " + this.phase));
        this.done = true; finish(this.checks); return;
    }
    let diagram = query("InspectConnectionDiagram");
    if diagram == () || diagram.built == false { return; }
    if diagram.nodes == () {
        this.done = true; finish([t_true(false, "diagram query failed: " + diagram)]); return;
    }
    if this.phase == 0 {
        if diagram.nodes.len() == 0 { return; }
        if ctx.expected_scope != () && diagram.scope != ctx.expected_scope {
            if ctx.open_key != () && this.open_request == () {
                this.open_request = cmd("OpenConnectionNode", #{ view_id: diagram.view_id, key: ctx.open_key });
            }
            return;
        }
        if this.expect_runtime_ports && !diagram.unresolved_links.is_empty() { return; }
        this.checks.push(t_true(diagram.unresolved_links.len() == 0, "referenced USD interfaces resolve through authored and runtime owners"));
        this.id = diagram.view_id;
        this.path = diagram.nodes[0].key;
        if ctx.expected_boundary_feedback != () {
            let inputs = (); let outputs = (); let feedback = 0;
            for node in diagram.nodes {
                if node.path == ctx.expected_scope && node.role == "inputs" { inputs = node; this.path = node.key; }
                if node.path == ctx.expected_scope && node.role == "outputs" { outputs = node; }
            }
            this.checks.push(t_true(inputs != () && outputs != () && inputs.key != outputs.key, "system interfaces have independent presentation identities"));
            let exact = true;
            for link in diagram.connections {
                if link.source == ctx.expected_scope && link.target == ctx.expected_scope {
                    feedback += 1;
                    if link.source_key == link.target_key { exact = false; }
                }
                if link.source.contains("#") || link.target.contains("#") { exact = false; }
            }
            this.checks.push(t_true(feedback.to_string() == ctx.expected_boundary_feedback.to_string() && exact, "authored same-prim feedback resolves through boundary terminals with exact USD origins"));
            this.checks.push(t_true(diagram.connection_count.to_string() == ctx.expected_connections.to_string(), "all scoped authored links remain rendered"));
        }
        this.source = diagram.source;
        this.checks.push(t_true(diagram.active_scene && diagram.source != (), "default diagram belongs to loaded USD scene"));
        cmd("CreateConnectionView", #{ view_id: this.id, name: "Diagram contract", scope: diagram.scope });
        this.phase = 1; return;
    }
    if this.phase == 1 {
        if diagram.view != "Diagram contract" { return; }
        cmd("MoveConnectionViewNode", #{ view_id: this.id, view: "Diagram contract", path: this.path, x: 10.0, y: 20.0 });
        this.phase = 2; return;
    }
    if this.phase == 2 {
        let view = definition(diagram, "Diagram contract");
        if view.positions.len() == 0 { return; }
        this.checks.push(t_true(view.positions[0].x == 10.0 && view.positions[0].y == 20.0, "view stores requested placement"));
        let displayed = ();
        for node in diagram.nodes { if node.key == this.path { displayed = node; } }
        this.checks.push(t_true(displayed != () && displayed.x == 10.0 && displayed.y == 20.0, "typed placement updates the displayed node"));
        this.checks.push(t_true(definition(diagram, "Overview").positions.len() == 0, "other named view retains independent automatic layout"));
        this.rejection = cmd("MoveConnectionViewNode", #{ view_id: this.id, view: "Diagram contract", path: "/AbsentDiagramPrim", x: 1.0, y: 2.0 });
        this.phase = 3; return;
    }
    if this.phase == 3 {
        let result = this.rejection;
        if result.status != "failed" && result.status != "rejected" { result = command_result(this.rejection.id); }
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "missing source node rejects without crashing"));
        this.checks.push(t_true(diagram.source == this.source && definition(diagram, "Diagram contract").positions.len() == 1, "rejection preserves source and valid layout"));
        this.rejection = cmd("OpenConnectionNode", #{ view_id: this.id, key: "/AbsentDiagramCard" });
        this.phase = 31; return;
    }
    if this.phase == 31 {
        let result = command_result(this.rejection.id);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "missing diagram card rejects navigation without crashing"));
        cmd("UndoConnectionView", #{ view_id: this.id, redo: false });
        this.phase = 4; return;
    }
    if this.phase == 4 {
        if definition(diagram, "Diagram contract").positions.len() != 0 { return; }
        this.checks.push(t_true(true, "layout undo restores automatic placement"));
        cmd("UndoConnectionView", #{ view_id: this.id, redo: true });
        this.phase = 5; return;
    }
    if this.phase == 5 {
        if definition(diagram, "Diagram contract").positions.len() != 1 { return; }
        this.checks.push(t_true(diagram.source == this.source, "layout redo preserves USD source binding"));
        if ctx.missing_view == () { this.done = true; finish(this.checks); return; }
        this.file_result = cmd("ConnectionViewFile", #{ view_id: this.id, path: ctx.saved_view, save: true, scope: diagram.scope });
        this.phase = 6; return;
    }
    if this.phase == 6 {
        let result = command_result(this.file_result.id);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.ok, "view file save completes through storage owner"));
        this.file_result = cmd("ConnectionViewFile", #{ view_id: this.id, path: ctx.missing_view, save: false, scope: diagram.scope });
        this.phase = 7; return;
    }
    if this.phase == 7 {
        let result = command_result(this.file_result.id);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.ok && diagram.source == this.source && diagram.definitions.len() == 1, "missing optional view file keeps source and automatic layout"));
        this.file_result = cmd("ConnectionViewFile", #{ view_id: this.id, path: ctx.saved_view, save: false, scope: diagram.scope });
        this.phase = 8; return;
    }
    if this.phase == 8 {
        let result = command_result(this.file_result.id);
        if result.status == "pending" { return; }
        this.checks.push(t_true(result.ok && definition(diagram, "Diagram contract").positions.len() == 1, "saved named placements roundtrip"));
        this.file_result = cmd("ConnectionViewFile", #{ view_id: this.id, path: ctx.corrupt_view, save: false, scope: diagram.scope });
        this.phase = 9; return;
    }
    if this.phase == 9 {
        let result = command_result(this.file_result.id);
        if result.status == "pending" { return; }
        let recovered = definition(diagram, "Recovered");
        this.checks.push(t_true(result.ok && recovered != () && recovered.positions.len() == 1, "corrupt file salvages a usable node position"));
        this.checks.push(t_true(diagram.source == this.source && diagram.error != () && diagram.built, "recovery warns without disrupting the USD diagram"));
        this.done = true; finish(this.checks);
    }
}
