// TEST-ONLY: verify composed variants, port sides and attached-model navigation.
const TEST_KIND = "editor";
fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [], query_reads: ["InspectConnectionDiagram", "ListOpenDocuments"], required_inputs: [] }
}
fn on_start(me, ctx) { this.phase = 0; this.elapsed = 0.0; this.done = false; this.checks = []; }
fn variant(diagram, name) {
    for choice in diagram.variants { if choice.name == name { return choice.selection; } }
    ()
}
fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.checks.push(t_true(false, "diagram context timeout phase " + this.phase));
        this.done = true; report_verdict(this.checks, "DIAGRAM CONTEXT", "DIAGRAM_CONTEXT"); return;
    }
    let diagram = query("InspectConnectionDiagram");
    if diagram == () || !diagram.built { return; }
    if this.phase == 0 {
        this.view = diagram.view_id;
        this.result = cmd("OpenConnectionNode", #{ view_id: this.view, key: ctx.system, program_path: ctx.program });
        this.phase = 1; return;
    }
    let result = command_result(this.result.id);
    if result.status == "pending" { return; }
    if this.phase == 1 {
        let opened = false;
        for doc in query("ListOpenDocuments").open_documents {
            if doc.kind == "modelica" && doc.title == ctx.model_title { opened = true; }
        }
        if result.status == "applied" && !opened { return; }
        this.checks.push(t_true(result.status == "applied" && opened, "explicit attached model opens its existing Modelica document"));
        cmd("FocusPanel", #{ id: "usd_connection_canvas" });
        this.result = cmd("OpenConnectionNode", #{ view_id: this.view, key: ctx.system });
        this.phase = 2; return;
    }
    if this.phase == 2 {
        if diagram.scope != ctx.system { return; }
        this.checks.push(t_true(diagram.variant_error == () && variant(diagram, "power") == "infinite", "system displays the composed infinite-power variant"));
        let battery_links = 0; let source_links = 0; let sides = true; let boundaries = 0; let model_style = false;
        for edge in diagram.connections {
            if edge.source == ctx.system + "/Battery" || edge.target == ctx.system + "/Battery" { battery_links += 1; }
            if edge.kind == "Acausal" && (edge.source == ctx.system + "/PowerSource" || edge.target == ctx.system + "/PowerSource") { source_links += 1; }
        }
        for node in diagram.nodes {
            if node.path == ctx.program && node.accent == "model" { model_style = true; }
            if node.role == "inputs" { if node.label != "System inputs" { return; } boundaries += 1; }
            if node.role == "outputs" { if node.label != "System outputs" { return; } boundaries += 1; }
            for port in node.ports {
                if node.role == "prim" && port.kind == "input" && port.x != node.x { sides = false; }
                if node.role == "prim" && port.kind == "output" && port.x <= node.x { sides = false; }
                if node.role == "inputs" && port.kind == "input" && port.x <= node.x { sides = false; }
                if node.role == "outputs" && port.kind == "output" && port.x != node.x { sides = false; }
            }
        }
        this.checks.push(t_true(battery_links == 0 && source_links == 4, "infinite-power topology wires motors to PowerSource and leaves Battery unused"));
        this.checks.push(t_true(sides && boundaries == 2, "causal sides and authored interface labels remain distinct"));
        this.checks.push(t_true(model_style, "Modelica card consumes the authored model color role"));
        this.result = cmd("CreateConnectionView", #{ view_id: this.view, name: "Variant context test", scope: ctx.battery_system });
        this.phase = 3; return;
    }
    if this.phase == 3 {
        if diagram.scope != ctx.battery_system { return; }
        let links = 0;
        for edge in diagram.connections {
            if edge.kind == "Acausal" && (edge.source == ctx.battery_system + "/Battery" || edge.target == ctx.battery_system + "/Battery") { links += 1; }
        }
        this.checks.push(t_true(variant(diagram, "power") == "battery" && links == 4, "battery variant exposes its four authored motor connections"));
        let torque_contract = false; let camera_contract = false; let flange_link = false;
        for node in diagram.nodes {
            if node.path == ctx.torque {
                let inputs = 0; let outputs = 0; let connectors = 0;
                for port in node.ports {
                    if port.kind == "input" { inputs += 1; }
                    if port.kind == "output" { outputs += 1; }
                    if port.kind == "acausal" { connectors += 1; }
                }
                torque_contract = inputs == 1 && outputs == 0 && connectors == 1;
            }
            if node.path == ctx.camera {
                let ports = 0;
                for port in node.ports { if port.kind != "joint" { ports += 1; } }
                camera_contract = ports == 0;
            }
        }
        for edge in diagram.connections {
            if edge.kind == "Acausal" && (edge.source == ctx.torque || edge.target == ctx.torque) { flange_link = true; }
        }
        this.checks.push(t_true(torque_contract && flange_link, "torque source has a causal command and a connected acausal flange, with no invented output"));
        this.checks.push(t_true(camera_contract, "USD camera remains visible without invented signal ports"));
        this.result = cmd("OpenConnectionNode", #{ view_id: this.view, key: ctx.battery_system + "/Battery", program_path: "/AbsentProgram" });
        this.phase = 4; return;
    }
    this.checks.push(t_true(result.status == "failed" || result.status == "rejected", "missing attached program rejects without opening an unrelated source"));
    this.done = true; report_verdict(this.checks, "DIAGRAM CONTEXT", "DIAGRAM_CONTEXT");
}
