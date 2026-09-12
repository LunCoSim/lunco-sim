// Authored route editing policy.
//
// Rust supplies only the generic click context and the typed USD-operation
// command. This policy creates ordinary USD prims and a generic Rhai program;
// no route, waypoint, or vehicle type is known by the engine.

fn ui_label() { "Route point" }
fn ui_hint() { "Click the terrain to append a point to the active route" }

fn failure(message) {
    notify_kind("Route edit failed: " + message, "error");
    #{ ok: false, error: message }
}

fn target_record(doc, path) {
    let result = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: "@root@",
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "USD target query returned no result" } else { result.error } };
    }
    #{ ok: true, exists: result.composed_exists == true || result.authored_in_document == true }
}

fn generation(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    if result == () || result.ok == false || result.generation == () {
        return ();
    }
    result.generation
}

fn prim_exists(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false
}

fn apply_ops(doc, generation_path, label, ops) {
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: generation(doc, generation_path),
        label: label,
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD edit returned no result" } else { result.error });
    }
    #{ ok: true }
}

fn route_path(context) {
    if context.route_path != () && context.route_path != "" {
        return context.route_path;
    }
    if context.scene_root_path == () || context.scene_root_path == "" {
        return ();
    }
    context.scene_root_path + "/Route"
}

fn subject_path(context) {
    if context.controlled_path != () && context.controlled_path != "" {
        return context.controlled_path;
    }
    if context.selected_path != () && context.selected_path != "" {
        return context.selected_path;
    }
    ()
}

fn next_point_name(doc, route) {
    let children = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    if children == () || children.ok == false || children.children == () {
        return "P0";
    }
    let prefix = "P";
    for child in children.children {
        let parts = child.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf.starts_with("W") { prefix = "W"; break; }
    }
    let index = 0;
    while index < 100000 {
        let candidate = prefix + index.to_string();
        let used = false;
        for child in children.children {
            if child == route + "/" + candidate { used = true; break; }
        }
        if !used { return candidate; }
        index += 1;
    }
    ()
}

fn route_positions(doc, route, appended) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    let positions = [];
    if result != () && result.ok != false && result.children != () {
        for child in result.children {
            let parts = child.split("/");
            let leaf = parts[parts.len() - 1];
            if leaf == "Program" { continue; }
            let point = query("QueryUsdPrim", #{
                doc_id: doc,
                path: child,
                attrs: ["xformOp:translate"],
            });
            if point != () && point.ok != false && point.attrs != ()
                && point.attrs["xformOp:translate"] != () {
                positions.push(point.attrs["xformOp:translate"]);
            }
        }
    }
    if appended != () { positions.push(appended); }
    positions
}

fn point_literal(point) {
    "(" + point[0].to_string() + ", "
        + point[1].to_string() + ", " + point[2].to_string() + ")"
}

fn points_literal(points) {
    let text = "[";
    for i in 0..points.len() {
        if i > 0 { text += ", "; }
        text += point_literal(points[i]);
    }
    text + "]"
}

fn ribbon_path(route) {
    let name = "RouteRibbon";
    for segment in route.split("/") {
        if segment != "" { name += "_" + segment; }
    }
    let parts = route.split("/");
    let scene_root = "";
    for i in 1..parts.len() - 1 {
        scene_root += "/" + parts[i];
    }
    scene_root + "/" + name
}

fn ribbon_geometry_path(route) {
    ribbon_path(route) + "/Geometry"
}

fn ribbon_ops(doc, route, points) {
    if points.len() < 2 { return []; }
    let ribbon = ribbon_path(route);
    let geometry = ribbon_geometry_path(route);
    let anchor = points[0];
    let local_points = [];
    for point in points {
        local_points.push([
            point[0] - anchor[0],
            point[1] - anchor[1],
            point[2] - anchor[2],
        ]);
    }
    let ops = [];
    if !prim_exists(doc, ribbon) {
        let parts = ribbon.split("/");
        let parent = "";
        for i in 1..parts.len() - 1 { parent += "/" + parts[i]; }
        ops.push(#{ AddPrim: #{
            edit_target: "@runtime@",
            parent_path: parent,
            name: parts[parts.len() - 1],
            type_name: "Xform",
            reference: "lunco://markers/route_ribbon.usda",
            reference_prim_path: "",
        }});
    }
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: ribbon,
        name: "xformOp:translate",
        type_name: "double3",
        value: point_literal(anchor),
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: ribbon,
        name: "xformOpOrder",
        type_name: "token[]",
        value: "[\"xformOp:translate\"]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "type",
        type_name: "token",
        value: "\"linear\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "wrap",
        type_name: "token",
        value: "\"nonperiodic\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "curveVertexCounts",
        type_name: "int[]",
        value: "[" + points.len().to_string() + "]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "points",
        type_name: "point3f[]",
        value: points_literal(local_points),
    }});
    ops
}

// Route points are authored facts. The ribbon is a disposable tool view in
// the document runtime layer, so a Twin remains portable and contains no
// editor-only presentation prim. A live scene may call this at program start
// to materialize the view for an already-authored route.
fn ensure_ribbon(context) {
    if context == () || context.doc_id == () || context.route_path == () {
        return failure("route view has no explicit document or route path");
    }
    let points = route_positions(context.doc_id, context.route_path, ());
    let ops = ribbon_ops(context.doc_id, context.route_path, points);
    if ops.len() == 0 { return #{ ok: true }; }
    apply_ops(context.doc_id, context.route_path, "Update route ribbon", ops)
}

fn route_setup(doc, route, subject) {
    let existing = target_record(doc, route);
    if !existing.ok { return existing; }
    let ops = [];
    if !existing.exists {
        if route.index_of("/") < 0 {
            return #{ ok: false, error: "route path must be rooted" };
        }
        let parts = route.split("/");
        let parent = "";
        for i in 1..parts.len() - 1 {
            parent += "/" + parts[i];
        }
        let name = parts[parts.len() - 1];
        ops.push(#{ AddPrim: #{
            edit_target: "@root@",
            parent_path: parent,
            name: name,
            type_name: "Scope",
            reference: (),
            reference_prim_path: "",
        }});
    }
    let program = route + "/Program";
    let program_record = target_record(doc, program);
    if !program_record.ok { return program_record; }
    if !program_record.exists {
        ops.push(#{ AddPrim: #{
            edit_target: "@root@",
            parent_path: route,
            name: "Program",
            type_name: "Scope",
            reference: (),
            reference_prim_path: "",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@root@",
            path: program,
            name: "inputs:enabled",
            type_name: "float",
            value: "0.0",
        }});
        ops.push(#{ SetApiSchemas: #{
            edit_target: "@root@",
            path: program,
            schemas: ["LunCoProgramAPI"],
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@root@",
            path: program,
            name: "info:implementationSource",
            type_name: "token",
            value: "\"sourceAsset\"",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@root@",
            path: program,
            name: "info:id",
            type_name: "token",
            value: "\"\"",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@root@",
            path: program,
            name: "info:sourceCode",
            type_name: "string",
            value: "",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@root@",
            path: program,
            name: "info:sourceAsset",
            type_name: "asset",
            value: "@lunco://scenarios/route_follow.rhai@",
        }});
        ops.push(#{ SetRelationship: #{
            edit_target: "@root@",
            path: program,
            name: "inputs:subject",
            targets: [subject],
        }});
    }
    #{ ok: true, ops: ops }
}

fn marker_ops(doc, route, position) {
    let name = next_point_name(doc, route);
    if name == () { return #{ ok: false, error: "route contains too many points" }; }
    let path = route + "/" + name;
    let zone = "route_" + name;
    #{ ok: true, path: path, ops: [
        #{ AddPrim: #{
            edit_target: "@root@",
            parent_path: route,
            name: name,
            type_name: "Xform",
            reference: "lunco://markers/route_point.usda",
            reference_prim_path: "",
        }},
        #{ SetTranslate: #{
            edit_target: "@root@",
            path: path,
            value: position,
        }},
        #{ SetAttribute: #{
            edit_target: "@root@",
            path: path + "/Trigger",
            name: "lunco:triggerZone",
            type_name: "token",
            value: "\"" + zone + "\"",
        }},
    ] }
}

fn on_click(context) {
    if context == () || context.doc_id == () || context.scene_root_path == () {
        return failure("click has no active document or scene root");
    }
    let subject = subject_path(context);
    if subject == () { return failure("select or possess the object that owns this route"); }
    if context.world_position == () { return failure("click has no world position"); }
    let route = route_path(context);
    if route == () { return failure("active scene root has no route path"); }
    let setup = route_setup(context.doc_id, route, subject);
    if !setup.ok { return failure(setup.error); }
    let point = marker_ops(context.doc_id, route, context.world_position);
    if !point.ok { return failure(point.error); }
    let ops = setup.ops;
    for op in point.ops { ops.push(op); }
    let positions = route_positions(context.doc_id, route, context.world_position);
    for op in ribbon_ops(context.doc_id, route, positions) { ops.push(op); }
    apply_ops(context.doc_id, context.scene_root_path, "Append route point", ops)
}

// Secondary pointer context is intentionally a safe selection action. The
// route point remains an ordinary USD prim: after selecting it, the standard
// transform gizmo and Delete command provide the generic editor operations.
// More specialised context actions can be added here without a Rust route
// observer.
fn on_context(context) {
    let entity_id = context.target_entity_id;
    if entity_id == () { entity_id = context.hit_entity_id; }
    if entity_id != () {
        cmd("SelectEntity", #{
            entity_id: entity_id,
            extend: false,
            toggle: false,
            remove_only: false,
        });
    }
}
