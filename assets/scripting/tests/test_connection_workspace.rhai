// Production workspace assertions; the driver supplies the exact owned view.
fn connection_workspace_view(mode, local_count) {
    let d = query("InspectConnectionDiagram");
    report_verdict([
        t_true(d.presentation == mode, "the requested authoring view is active"),
        t_true(d.local_connection_count == local_count, "same-prim bindings remain inspectable"),
        t_true(d.connection_count == d.connections.len(), "presentation retains every scoped connection"),
    ], "CONNECTION WORKSPACE", "CONNECTION_WORKSPACE");
}

fn connection_workspace_preview() {
    let d = query("InspectConnectionDiagram");
    let honest = true;
    for link in d.connections {
        if link.status != "document" || link.source_value != () || link.animating {
            honest = false;
        }
    }
    report_verdict([
        t_true(d.editing, "the USD document owns authoring"),
        t_true(honest, "an inert document never fabricates live values or traffic"),
    ], "CONNECTION DOCUMENT STATUS", "CONNECTION_WORKSPACE");
}

fn connection_workspace_binding_count(expected) {
    let d = query("InspectConnectionDiagram");
    report_verdict([t_true(d.connection_count == expected,
        "manual edits and undo update the authoritative USD topology")],
        "CONNECTION AUTHORING", "CONNECTION_WORKSPACE");
}

fn connection_workspace_drag_route(first_bend_x, last_bend_y) {
    let d = query("InspectConnectionDiagram");
    let route = ();
    for link in d.connections {
        if link.source == "/World/Source" && link.target == "/World/Sink" {
            route = link.route;
        }
    }
    let orthogonal = route != () && route.len() >= 4;
    if orthogonal {
        for i in 1..route.len() {
            let a = route[i - 1]; let b = route[i];
            orthogonal = orthogonal && (a.x == b.x || a.y == b.y);
        }
    }
    report_verdict([
        t_true(orthogonal, "the complete live wire stays orthogonal during the drag"),
        t_true(orthogonal && route[1].x != first_bend_x && route[route.len() - 2].y != last_bend_y,
            "interior bends follow the moved card before the gesture commits"),
    ], "CONNECTION DRAG ROUTE", "CONNECTION_WORKSPACE");
}

fn connection_workspace_exact_bindings(expected) {
    let d=query("InspectConnectionDiagram");let actual=[];
    for link in d.connections {actual.push([link.source+"."+link.source_port,link.target+"."+link.target_port]);}
    let exact=actual.len()==expected.len();
    for pair in expected {exact=exact && actual.contains(pair);}
    report_verdict([t_true(exact,"binding edits preserve exactly the requested topology and unrelated sources")],
        "CONNECTION EDIT TOPOLOGY","CONNECTION_WORKSPACE");
}

fn connection_workspace_rejection(before) {
    let d = query("InspectConnectionDiagram");
    report_verdict([
        t_true(d.connection_count == before, "a rejected edit preserves existing connections"),
        t_true(query("GetReadiness").application_available, "invalid authored input does not crash the application"),
    ], "CONNECTION REJECTION", "CONNECTION_WORKSPACE");
}

fn connection_workspace_live(previous, minimum, maximum) {
    let d=query("InspectConnectionDiagram");
    let bound=true;let values=[];let finite=true;
    for link in d.connections {
        bound=bound && link.status=="bound";
        values.push(link.source_value);
        finite=finite && link.source_value!=() && link.source_value>=minimum && link.source_value<=maximum;
    }
    report_verdict([
        t_true(d.active_scene && bound,"runtime binding state comes from the live owner"),
        t_true(finite && values!=previous,"the selected Modelica class supplies advancing source samples"),
    ],"CONNECTION LIVE VALUES","CONNECTION_WORKSPACE");
}

fn connection_workspace_paused() {
    let d=query("InspectConnectionDiagram");
    let stopped=true;for link in d.connections {stopped=stopped && !link.animating;}
    report_verdict([t_true(stopped,"paused simulation has no observed-change animation")],
        "CONNECTION PAUSED","CONNECTION_WORKSPACE");
}

fn connection_workspace_forwarded() {
    let d = query("InspectConnectionDiagram");
    let bound = false;
    for link in d.connections {
        if link.target == "/World/Relay" && link.target_port == "outputs:forwarded" {
            bound = link.status == "bound" && link.target_value != () &&
                link.target_value >= 5.0 && link.target_value <= 7.0;
        }
    }
    report_verdict([t_true(bound, "an authored output forward shows its resolved runtime status and sample")],
        "CONNECTION FORWARDED OUTPUT", "CONNECTION_WORKSPACE");
}

fn connection_workspace_held() {
    let d=query("InspectConnectionDiagram");let held=false;
    for link in d.connections {
        if link.target=="/World/Sink" {
            held=link.status=="held" && link.target_value==2.75 && !link.animating;
        }
    }
    report_verdict([t_true(held,"an overriding input hold shows its own value and suppresses the wire highlight")],
        "CONNECTION HELD","CONNECTION_WORKSPACE");
}

fn connection_workspace_icon() {
    let d=query("InspectConnectionDiagram");let exact=false;
    for node in d.nodes {
        if node.path=="/World/Source" {exact=node.has_modelica_icon && node.icon_program=="/World/Source";}
    }
    report_verdict([t_true(exact,"the exact attached Modelica source supplies its authored icon")],
        "CONNECTION MODEL ICON","CONNECTION_WORKSPACE");
}

fn connection_workspace_model_source(doc, marker) {
    let source=query("GetDocumentSource",#{doc_id:doc});
    report_verdict([t_true(source.source.contains(marker),
        "the existing Modelica document owner applies and restores source edits")],
        "CONNECTION MODEL EDITOR","CONNECTION_WORKSPACE");
}

fn connection_workspace_model_class(doc, expected) {
    let model = query("CompileStatus", #{doc_id: doc});
    report_verdict([t_true(model.drilled_in_class == expected,
        "opening an attached program selects the exact class authored in USD")],
        "CONNECTION MODEL CLASS", "CONNECTION_WORKSPACE");
}

fn connection_workspace_grouped(rendered) {
    let d=query("InspectConnectionDiagram");
    report_verdict([
        t_true(d.connection_count==2 && d.connections.len()==2,"group disclosure preserves both exact bindings"),
        t_true(d.local_connection_count==1 && d.rendered_connection_count==rendered,"local summaries and group disclosure compose"),
    ],"CONNECTION GROUP DISCLOSURE","CONNECTION_WORKSPACE");
}

fn connection_workspace_dense() {
    let d=query("InspectConnectionDiagram");let ports=0;let local=0;
    for node in d.nodes {
        if node.path!="/World/Dense" {continue;}
        local=node.local_binding_count;
        for port in node.ports {if port.kind=="input" || port.kind=="output" {ports+=1;}}
    }
    report_verdict([
        t_true(ports==80 && local==40,"one dense model retains every long input/output identity"),
        t_true(d.connection_count==42 && d.local_connection_count==41,"scrolled views retain all dense and ordinary bindings"),
    ],"CONNECTION DENSE VIEW","CONNECTION_WORKSPACE");
}
