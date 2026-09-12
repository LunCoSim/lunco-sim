// Authored route editing policy.
//
// Rust supplies only the generic pointer context and typed USD-operation
// command. This policy creates ordinary USD prims and a disposable route-view
// overlay; no route, waypoint, or vehicle type is known by the engine.

fn ui_label() { "Route point" }
fn ui_hint() { "Alt-click the terrain to append a point to the active route" }

fn failure(message) {
    notify_kind("Route edit failed: " + message, "error");
    #{ ok: false, error: message }
}

fn target_record(doc, path, edit_target) {
    let result = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: edit_target,
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "USD target query returned no result" } else { result.error } };
    }
    #{
        ok: true,
        exists: result.composed_exists == true || result.authored_in_document == true,
        authored_here: result.authored_here == true,
        edit_scope: result.edit_scope,
        under_arc: result.under_arc == true,
    }
}

fn generation(doc) {
    let result = query("InspectUsdDocument", #{ doc_id: doc });
    if result == () || result.ok == false || result.generation == () {
        return ();
    }
    result.generation
}

fn prim_exists(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false
}

fn apply_ops(doc, label, ops) {
    if ops.len() == 0 { return #{ ok: true, changed: false }; }
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: generation(doc),
        label: label,
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD edit returned no result" } else { result.error });
    }
    #{ ok: true, changed: true }
}

fn route_edit_path(context) {
    let explicit = context["route_path"];
    if explicit != () && explicit != "" {
        return explicit;
    }
    let scene_root = context["scene_root_path"];
    if scene_root == () || scene_root == "" {
        return ();
    }
    scene_root + "/Route"
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

fn direct_route_point(path, route) {
    if path == () || path == "" || route == () || route == "" {
        return false;
    }
    let prefix = route + "/";
    if !path.starts_with(prefix) { return false; }
    let remainder = path.sub_string(prefix.len());
    remainder != "" && remainder.index_of("/") < 0 && remainder != "Program"
}

fn context_route_point(context, route) {
    if context == () { return (); }
    let candidates = [];
    if context.target_path != () { candidates.push(context.target_path); }
    if context.hit_path != () { candidates.push(context.hit_path); }
    if context.prim_paths != () {
        for path in context.prim_paths { candidates.push(path); }
    }
    for path in candidates {
        if direct_route_point(path, route) { return path; }
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

// Read the route points once and optionally replace or omit one point. This
// keeps a move/delete and its ribbon refresh in the same USD transaction.
fn route_positions(doc, route, edited_path, replacement, appended) {
    let document_gen = generation(doc);
    if document_gen == () {
        return #{ ok: false, error: "route document generation is unavailable" };
    }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    let positions = [];
    if result == () || result.ok == false || result.children == () {
        return #{ ok: false, error: "route stage is not synchronized" };
    }
    if result.generation != document_gen {
        return #{ ok: false, error: "route stage is not synchronized" };
    }
    for child in result.children {
        let parts = child.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf == "Program" { continue; }
        let record = query("QueryUsdPrim", #{ doc_id: doc, path: child });
        if record == () || record.ok == false {
            return #{ ok: false, error: "route point state is not synchronized" };
        }
        if record.generation != document_gen {
            return #{ ok: false, error: "route point state is not synchronized" };
        }
        if record.active != true { continue; }
        if child == edited_path {
            if replacement != () { positions.push(replacement); }
            continue;
        }
        let point = query("QueryUsdPrim", #{
            doc_id: doc,
            path: child,
            attrs: ["xformOp:translate"],
        });
        if point == () || point.ok == false || point.attrs == ()
            || point.attrs["xformOp:translate"] == () {
            return #{ ok: false, error: "route point has no authored position" };
        }
        positions.push(point.attrs["xformOp:translate"]);
    }
    if appended != () { positions.push(appended); }
    #{ ok: true, points: positions }
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

fn vector_equal(a, b) {
    if a == () || b == () || a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if (a[i] - b[i]).abs() > 0.000001 { return false; }
    }
    true
}

fn points_equal(a, b) {
    if a == () || b == () || a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if !vector_equal(a[i], b[i]) { return false; }
    }
    true
}

fn ribbon_path(route) {
    let name = "RouteRibbon";
    for segment in route.split("/") {
        if segment != "" { name += "_" + segment; }
    }
    let parts = route.split("/");
    let scene_root = "";
    for i in 1..parts.len() - 1 { scene_root += "/" + parts[i]; }
    scene_root + "/" + name
}

fn ribbon_geometry_path(route) {
    ribbon_path(route) + "/Geometry"
}

fn ribbon_needs_update(doc, ribbon, geometry, anchor, local_points, count) {
    if !prim_exists(doc, ribbon) { return true; }
    let root = query("QueryUsdPrim", #{
        doc_id: doc,
        path: ribbon,
        attrs: ["xformOp:translate"],
    });
    let shape = query("QueryUsdPrim", #{
        doc_id: doc,
        path: geometry,
        attrs: ["type", "wrap", "curveVertexCounts", "points"],
    });
    if root == () || root.ok == false || root.attrs == ()
        || shape == () || shape.ok == false || shape.attrs == () {
        return true;
    }
    if root.attrs["xformOp:translate"] == ()
        || !vector_equal(root.attrs["xformOp:translate"], anchor)
        || shape.attrs["type"] != "linear"
        || shape.attrs["wrap"] != "nonperiodic"
        || shape.attrs["curveVertexCounts"] != [count]
        || !points_equal(shape.attrs["points"], local_points) {
        return true;
    }
    false
}

fn ribbon_ops(doc, route, points) {
    let ribbon = ribbon_path(route);
    if points.len() < 2 {
        if prim_exists(doc, ribbon) {
            return [#{ RemovePrim: #{
                edit_target: "@runtime@",
                path: ribbon,
            }}];
        }
        return [];
    }
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
    if !ribbon_needs_update(doc, ribbon, geometry, anchor, local_points, points.len()) {
        return [];
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
// the runtime layer, so a Twin remains portable and contains no editor-only
// presentation prim.
fn ensure_ribbon(context) {
    if context == () || context.doc_id == () || context.route_path == () {
        return failure("route view has no explicit document or route path");
    }
    let read = route_positions(context.doc_id, context.route_path, (), (), ());
    if read.ok != true { return read; }
    let ops = ribbon_ops(context.doc_id, context.route_path, read.points);
    apply_ops(context.doc_id, "Update route ribbon", ops)
}

fn route_setup(doc, route, subject) {
    let existing = target_record(doc, route, "@root@");
    if !existing.ok { return existing; }
    let ops = [];
    if !existing.exists {
        if route.index_of("/") < 0 {
            return #{ ok: false, error: "route path must be rooted" };
        }
        let parts = route.split("/");
        let parent = "";
        for i in 1..parts.len() - 1 { parent += "/" + parts[i]; }
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
    let program_record = target_record(doc, program, "@root@");
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
        // `sourceAsset` is the complete program source contract. Do not write
        // conflicting `info:id` or `info:sourceCode` opinions beside it.
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
    } else {
        let links = query("QueryUsdPrim", #{
            doc_id: doc,
            path: program,
            rels: ["inputs:subject"],
        });
        let targets = if links != () && links.ok != false
            && links.relationships != () { links.relationships["inputs:subject"] } else { () };
        if targets == () || targets.len() == 0 {
            ops.push(#{ SetRelationship: #{
                edit_target: "@root@",
                path: program,
                name: "inputs:subject",
                targets: [subject],
            }});
        } else if targets[0] != subject {
            return #{ ok: false, error: "route is already assigned to another subject" };
        }
    }
    #{ ok: true, created: !existing.exists, ops: ops }
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
    let route = route_edit_path(context);
    if route == () { return failure("active scene root has no route path"); }
    let setup = route_setup(context.doc_id, route, subject);
    if !setup.ok { return failure(setup.error); }
    let point = marker_ops(context.doc_id, route, context.world_position);
    if !point.ok { return failure(point.error); }
    let ops = setup.ops;
    for op in point.ops { ops.push(op); }
    let positions = if setup.created {
        #{ ok: true, points: [context.world_position] }
    } else {
        let read = route_positions(context.doc_id, route, (), (), context.world_position);
        if read.ok != true { return failure(read.error); }
        read
    };
    let view_ops = ribbon_ops(context.doc_id, route, positions.points);
    for op in view_ops { ops.push(op); }
    let result = apply_ops(context.doc_id, "Append route point", ops);
    if result.ok { result.path = point.path; }
    result
}

fn on_move(context) {
    if context == () || context.doc_id == () {
        return failure("move has no active document");
    }
    if context.world_position == () { return failure("move has no target position"); }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("move target is not a direct route point"); }
    let target = target_record(context.doc_id, point, "@root@");
    if !target.ok || !target.exists { return failure("move target does not exist"); }
    let edit_target = if target.authored_here || target.under_arc { "@root@" } else { "@runtime@" };
    let ops = [#{ SetTranslate: #{
        edit_target: edit_target,
        path: point,
        value: context.world_position,
    }}];
    let read = route_positions(context.doc_id, route, point, context.world_position, ());
    if read.ok != true { return failure(read.error); }
    for op in ribbon_ops(context.doc_id, route, read.points) { ops.push(op); }
    let result = apply_ops(context.doc_id, "Move route point", ops);
    if result.ok { result.path = point; }
    result
}

fn on_delete(context) {
    if context == () || context.doc_id == () {
        return failure("delete has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("delete target is not a direct route point"); }
    let target = target_record(context.doc_id, point, "@root@");
    if !target.ok || !target.exists { return failure("delete target does not exist"); }

    let op = if target.authored_here {
        #{ RemovePrim: #{ edit_target: "@root@", path: point } }
    } else if target.under_arc {
        // A referenced child cannot be removed from the referencing layer. USD
        // expresses this edit as a stronger local active=false opinion.
        #{ SetActive: #{ edit_target: "@root@", path: point, active: false } }
    } else {
        let runtime = target_record(context.doc_id, point, "@runtime@");
        if runtime.ok && runtime.authored_here {
            #{ RemovePrim: #{ edit_target: "@runtime@", path: point } }
        } else {
            return failure("route point is composed read-only");
        }
    };
    let read = route_positions(context.doc_id, route, point, (), ());
    if read.ok != true { return failure(read.error); }
    let ops = [op];
    for ribbon_op in ribbon_ops(context.doc_id, route, read.points) { ops.push(ribbon_op); }
    let result = apply_ops(context.doc_id, "Delete route point", ops);
    if result.ok { result.path = point; }
    result
}

// Secondary pointer context is a safe, route-scoped selection action. The
// route point remains an ordinary USD prim; the standard gizmo and Delete
// command can operate on the selected entity.
fn on_context(context) {
    if context == () || context.doc_id == () {
        return failure("context click has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("context target is not a direct route point"); }
    let entity_id = context.target_entity_id;
    if entity_id == () { entity_id = context.hit_entity_id; }
    if entity_id == () { return failure("route point has no selectable entity"); }
    let result = cmd("SelectEntity", #{
        entity_id: entity_id,
        extend: false,
        toggle: false,
        remove_only: false,
    });
    if result == () {
        return failure("SelectEntity command is unavailable in this host");
    }
    if result.ok == false {
        return failure(result.error);
    }
    #{ ok: true, path: point }
}
