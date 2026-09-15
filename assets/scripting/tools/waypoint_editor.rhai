// Authored route editing policy.
//
// Rust supplies only the generic pointer context and typed USD-operation
// command. This policy creates ordinary USD prims and a disposable route-view
// overlay; no route, waypoint, or vehicle type is known by the engine.

// Module functions are used instead of top-level variables because imported
// Rhai tool functions execute in their own module scope.
fn route_ribbon_width() { 0.12 }
fn route_ribbon_clearance() { 0.03 }
// Keep the transient BasisCurves payload below the Rhai/USD operation limit
// while still sampling more densely than the original long-leg chord
// approximation.
fn route_ribbon_sample_spacing() { 3.0 }
// A transient USD attribute is lowered through a bounded string payload. Keep
// the whole route below that transport limit; long legs increase their sample
// spacing adaptively while every authored waypoint remains an endpoint.
fn route_ribbon_max_samples() { 768 }

// Marker appearance is a generic route-tool policy. The asset carries the
// unvisited colour in standard gprim displayColor; the route program applies
// the visited colour through the transient USD view path when its generic
// sensor event advances the route.
fn unvisited_marker_color() { [0.10, 0.95, 0.22] }
fn visited_marker_color() { [0.55, 0.55, 0.55] }

fn ui_label() { "Route point" }
fn ui_hint() { "Use the configured add-point gesture on terrain to append a point to the active route" }

// The generic scene adapter invokes this hook for an unarmed pointer gesture.
// Physical button/modifier meaning is resolved by shared input settings; this
// policy consumes only semantic pointer intents.
fn on_pointer(context) {
    if context == () { return #{ ok: true, changed: false, handled: false }; }
    if pointer_intent(context, "route.add_point") {
        let point = pointer_point(context);
        if point.ok != true { return failure(point.error); }
        return on_click(context);
    }
    if pointer_intent(context, "route.context") {
        let route = route_edit_path(context);
        if route == () || context_route_point(context, route) == () {
            return #{ ok: true, changed: false, handled: false };
        }
        return on_context(context);
    }
    #{ ok: true, changed: false, handled: false }
}

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

fn apply_view_ops(doc, label, ops) {
    if ops.len() == 0 { return #{ ok: true, changed: false }; }
    let result = cmd("ApplyUsdTransientOps", #{
        doc_id: doc,
        parent_gen: generation(doc),
        label: label,
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD view edit returned no result" } else { result.error });
    }
    #{ ok: true, changed: true }
}

fn set_marker_state(doc, point, visited) {
    if doc == () || point == () || point == "" {
        return #{ ok: false, error: "route-point state has no document or point path" };
    }
    let dome = point + "/Dome";
    let current = query("QueryUsdPrim", #{
        doc_id: doc,
        path: dome,
        attrs: ["primvars:displayColor"],
    });
    if current == () || current.ok == false || current.attrs == ()
        || current.attrs["primvars:displayColor"] == () {
        return #{ ok: false, retryable: true,
            error: "route-point marker display state is not synchronized" };
    }
    let color = if visited { visited_marker_color() } else { unvisited_marker_color() };
    let current_color = current.attrs["primvars:displayColor"];
    if current_color.len() == 1 && vector_equal(current_color[0], color) {
        return #{ ok: true, changed: false };
    }
    apply_view_ops(doc, "Update route-point state", [#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: dome,
        name: "primvars:displayColor",
        type_name: "color3f[]",
        value: "[" + point_literal(color) + "]",
    }}])
}

fn reset_marker_states(doc, route) {
    let points = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    if points == () || points.ok == false || points.children == () {
        return #{ ok: false, retryable: true,
            error: "route-point state cannot read the route children" };
    }
    let ops = [];
    let color = unvisited_marker_color();
    for point in points.children {
        let parts = point.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf == "Program" || leaf == "RouteRibbon" { continue; }
        let marker = query("QueryUsdPrim", #{
            doc_id: doc,
            path: point + "/Dome",
            attrs: ["primvars:displayColor"],
        });
        if marker == () || marker.ok == false || marker.attrs == ()
            || marker.attrs["primvars:displayColor"] == () {
            return #{ ok: false, retryable: true,
            error: "route-point marker display state is not synchronized" };
        }
        let current_color = marker.attrs["primvars:displayColor"];
        if current_color.len() != 1 || !vector_equal(current_color[0], color) {
            ops.push(#{ SetAttribute: #{
                edit_target: "@runtime@",
                path: point + "/Dome",
                name: "primvars:displayColor",
                type_name: "color3f[]",
                value: "[" + point_literal(color) + "]",
            }});
        }
    }
    apply_view_ops(doc, "Reset route-point states", ops)
}

fn route_edit_path(context) {
    let explicit = context["route_path"];
    if explicit != () && explicit != "" {
        return explicit;
    }
    let doc = context["doc_id"];
    let subject = subject_path(context);
    if doc == () { return (); }

    // A selected program is the unambiguous route identity. This is also how
    // the dynamic program HUD selects between several programs controlling the
    // same subject; no scene-root or vehicle-name convention is involved.
    let selected = [
        context["selected_path"],
        context["target_path"],
        context["hit_path"],
    ];
    for path in selected {
        let route = route_for_path(doc, path, subject);
        if route != () { return route; }
    }

    // Twin-authored bindings may expose one or more programs from the subject
    // through the standard relationship used by this policy. When no subject
    // is selected, the same program-side inputs:subject bindings are discovered
    // under the supplied scene root. One binding is enough for an
    // unattended/one-route workflow; more than one requires the generic
    // selected program entity so an edit cannot silently land in the wrong
    // route.
    let linked = linked_routes(doc, subject, context["scene_root_path"]);
    if linked.len() == 1 { return linked[0]; }
    if linked.len() > 1 { return (); }

    // A multi-program subject must first select one of those program prims
    // through the generic scene selection surface.
    ()
}

fn route_parent_path(path) {
    if path == () || path == "" { return (); }
    let parts = path.split("/");
    if parts.len() <= 2 { return "/"; }
    let last = parts.len() - 1;
    let result = "";
    for i in 1..last {
        if i > 1 { result += "/"; }
        result += parts[i];
    }
    "/" + result
}

fn is_route_program(doc, path, subject) {
    if path == () || path == "" { return false; }
    let record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        rels: ["inputs:subject"],
        schemas: true,
    });
    if record == () || record.ok == false || record.api_schemas == () {
        return false;
    }
    let program = false;
    for schema in record.api_schemas {
        if schema == "LunCoProgramAPI" { program = true; break; }
    }
    if !program || record.relationships == () { return false; }
    let targets = record.relationships["inputs:subject"];
    targets != () && targets.len() == 1
        && (subject == () || targets[0] == subject)
}

fn route_for_path(doc, path, subject) {
    if path == () || path == "" { return (); }
    let current = path;
    for depth in 0..32 {
        if is_route_program(doc, current, subject) {
            return route_parent_path(current);
        }
        if current == "/" { break; }
        current = route_parent_path(current);
        if current == () { break; }
    }
    ()
}

fn discover_subject_programs(doc, scope, subject, depth) {
    let programs = [];
    if scope == () || scope == "" || depth > 32 { return programs; }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: scope,
        children: true,
    });
    if result == () || result.ok == false || result.children == () {
        return programs;
    }
    for child in result.children {
        if is_route_program(doc, child, subject) {
            programs.push(child);
        }
        let nested = discover_subject_programs(doc, child, subject, depth + 1);
        for program in nested {
            if !programs.contains(program) { programs.push(program); }
        }
    }
    programs
}

fn linked_routes(doc, subject, scene_root) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: subject,
        rels: ["programs"],
    });
    let routes = [];
    let programs = [];
    if result != () && result.ok != false && result.relationships != () {
        let linked = result.relationships["programs"];
        if linked != () {
            for program in linked { programs.push(program); }
        }
    }
    // The standard subject-side relationship is authoritative when present.
    // A composed stage can briefly expose the program prim before that reverse
    // relationship has projected; inspect the authored program bindings in the
    // supplied scene scope for that one pointer transaction instead of making
    // the user select a program or inventing a route path.
    if programs.len() == 0 {
        programs = discover_subject_programs(doc, scene_root, subject, 0);
    }
    for program in programs {
        let route = route_for_path(doc, program, subject);
        if route != () && !routes.contains(route) { routes.push(route); }
    }
    routes
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
    remainder != "" && remainder.index_of("/") < 0
        && remainder != "Program" && remainder != "RouteRibbon"
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
        return ();
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

// Read the route points from one settled composed-stage generation. The
// result feeds only the disposable ribbon view; authored route points remain
// the document's durable state.
fn route_positions(doc, route) {
    let document_gen = generation(doc);
    if document_gen == () {
        return #{ ok: false, retryable: true,
            error: "route document generation is unavailable" };
    }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    let positions = [];
    if result == () || result.ok == false || result.children == () {
        return #{ ok: false, retryable: true, error: "route stage is not synchronized" };
    }
    if result.generation != document_gen {
        return #{ ok: false, retryable: true, error: "route stage is not synchronized" };
    }
    for child in result.children {
        let parts = child.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf == "Program" || leaf == "RouteRibbon" { continue; }
        let record = query("QueryUsdPrim", #{ doc_id: doc, path: child });
        if record == () || record.ok == false {
            return #{ ok: false, retryable: true,
                error: "route point state is not synchronized" };
        }
        if record.generation != document_gen {
            return #{ ok: false, retryable: true,
                error: "route point state is not synchronized" };
        }
        if record.active != true { continue; }
        let point = query("QueryUsdPrim", #{
            doc_id: doc,
            path: child,
            attrs: ["xformOp:translate"],
        });
        if point == () || point.ok == false || point.attrs == ()
            || point.attrs["xformOp:translate"] == () {
            return #{ ok: false, retryable: true,
                error: "route point has no authored position" };
        }
        positions.push(point.attrs["xformOp:translate"]);
    }
    #{ ok: true, points: positions }
}

// Sample the authoritative DEM for presentation points. If a Twin has no
// terrain oracle (for example a flat unit-test fixture), retain the authored
// route point and an up normal; that is the documented geometry of that scene,
// not a guessed origin or a vehicle-specific fallback.
fn terrain_ribbon_sample(point) {
    let result = query("TerrainHeight", #{
        x: point[0],
        z: point[2],
        eps: 1.0,
    });
    if result != () && result.found == true
        && result.height != () && result.normal != () {
        let normal = vnorm(result.normal);
        if normal != () {
            return #{
                point: [point[0], result.height, point[2]],
                normal: normal,
                surface: true,
            };
        }
    }
    #{ point: point, normal: [0.0, 1.0, 0.0], surface: false }
}

fn ribbon_step_count(a, b, spacing) {
    let distance = vlen([b[0] - a[0], 0.0, b[2] - a[2]]);
    let steps = 1;
    while steps.to_float() * spacing < distance {
        steps += 1;
    }
    steps
}

fn ribbon_sample_count(points, spacing) {
    let count = 1;
    for i in 0..(points.len() - 1) {
        count += ribbon_step_count(points[i], points[i + 1], spacing);
    }
    count
}

fn ribbon_sampling_spacing(points) {
    let spacing = route_ribbon_sample_spacing();
    if points.len() < 2 { return spacing; }

    let limit = if points.len() > route_ribbon_max_samples() {
        points.len()
    } else {
        route_ribbon_max_samples()
    };
    let count = ribbon_sample_count(points, spacing);
    // Keep ordinary routes at the authored 3 m quality. Only oversized
    // payloads trade density for a valid typed USD operation.
    while count > limit {
        spacing *= 1.25;
        count = ribbon_sample_count(points, spacing);
    }
    spacing
}

// Densify long authored legs and project each sample onto the DEM. This keeps
// the ribbon a terrain-following annotation instead of a sparse straight chord
// between distant waypoints. It runs only when the disposable view is rebuilt,
// never in the per-frame route-control task.
fn ribbon_surface_samples(points) {
    let sampled_points = [];
    let sampled_normals = [];
    if points.len() == 0 {
        return #{ points: sampled_points, normals: sampled_normals };
    }
    if points.len() == 1 {
        let sample = terrain_ribbon_sample(points[0]);
        sampled_points.push(sample.point);
        sampled_normals.push(sample.normal);
        return #{ points: sampled_points, normals: sampled_normals };
    }
    let first = terrain_ribbon_sample(points[0]);
    if first.surface != true {
        for point in points {
            sampled_points.push(point);
            sampled_normals.push([0.0, 1.0, 0.0]);
        }
        return #{ points: sampled_points, normals: sampled_normals };
    }
    let spacing = ribbon_sampling_spacing(points);
    for i in 0..(points.len() - 1) {
        let a = points[i];
        let b = points[i + 1];
        let steps = ribbon_step_count(a, b, spacing);
        for j in 0..steps {
            let t = j.to_float() / steps.to_float();
            let point = [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ];
            let sample = terrain_ribbon_sample(point);
            sampled_points.push(sample.point);
            sampled_normals.push(sample.normal);
        }
    }
    let last = terrain_ribbon_sample(points[points.len() - 1]);
    sampled_points.push(last.point);
    sampled_normals.push(last.normal);
    #{ points: sampled_points, normals: sampled_normals }
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

fn ribbon_normals_literal(input_normals) {
    let literals = [];
    for normal in input_normals { literals.push(normal); }
    points_literal(literals)
}

fn ribbon_clearanced_point(point, normal) {
    [
        point[0] + normal[0] * route_ribbon_clearance(),
        point[1] + normal[1] * route_ribbon_clearance(),
        point[2] + normal[2] * route_ribbon_clearance(),
    ]
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

fn ribbon_quantized_vectors(vectors, scale) {
    let quantized = [];
    for vector in vectors {
        quantized.push([
            (vector[0] * scale).round() / scale,
            (vector[1] * scale).round() / scale,
            (vector[2] * scale).round() / scale,
        ]);
    }
    quantized
}

fn ribbon_path(route) {
    // Keep the generated view under the route scope. Route points and their
    // annotation then share the same USD parent frame, including when a Twin
    // places the route below a transformed scene scope.
    route + "/RouteRibbon"
}

fn ribbon_geometry_path(route) {
    ribbon_path(route) + "/Geometry"
}

fn ribbon_width_matches(widths) {
    widths != () && widths.len() == 1
        && (widths[0] - route_ribbon_width()).abs() < 0.001
}

fn ribbon_needs_update(doc, ribbon, geometry, anchor, local_points, local_normals) {
    if !prim_exists(doc, ribbon) { return true; }
    let root = query("QueryUsdPrim", #{
        doc_id: doc,
        path: ribbon,
        attrs: ["xformOp:translate"],
    });
    let shape = query("QueryUsdPrim", #{
        doc_id: doc,
        path: geometry,
        attrs: ["type", "wrap", "curveVertexCounts", "points", "widths", "normals"],
    });
    if root == () || root.ok == false || root.attrs == ()
        || shape == () || shape.ok == false || shape.attrs == () {
        return true;
    }
    if root.attrs["xformOp:translate"] == ()
        || !vector_equal(root.attrs["xformOp:translate"], anchor)
        || shape.attrs["type"] != "linear"
        || shape.attrs["wrap"] != "nonperiodic"
        || shape.attrs["curveVertexCounts"] != [local_points.len()]
        || !ribbon_width_matches(shape.attrs["widths"])
        || !points_equal(shape.attrs["points"], local_points)
        || !points_equal(shape.attrs["normals"], local_normals) {
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
    let sampled = ribbon_surface_samples(points);
    let surface_points = sampled.points;
    let surface_normals = sampled.normals;
    if surface_points.len() < 2 || surface_normals.len() != surface_points.len() {
        return [];
    }
    let geometry = ribbon_geometry_path(route);
    // Keep the route root at the exact first sampled surface point so its USD
    // identity remains aligned with P0. Offset each curve vertex along the
    // sampled support normal instead of translating the child geometry in a
    // global axis; this keeps the narrow strip above slopes and steps.
    let anchor = surface_points[0];
    let local_points = [];
    for i in 0..surface_points.len() {
        let point = ribbon_clearanced_point(surface_points[i], surface_normals[i]);
        local_points.push([
            point[0] - anchor[0],
            point[1] - anchor[1],
            point[2] - anchor[2],
        ]);
    }
    // Compact only the transient USD text payload. Millimetre positions and
    // 0.1 mm normals keep the terrain-following shape while leaving ample
    // room below the Rhai string limit for long routes.
    let encoded_points = ribbon_quantized_vectors(local_points, 1000.0);
    let encoded_normals = ribbon_quantized_vectors(surface_normals, 10000.0);
    if !ribbon_needs_update(
        doc, ribbon, geometry, anchor, encoded_points, encoded_normals
    ) {
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
    ops.push(#{ SetTranslate: #{
        edit_target: "@runtime@",
        path: ribbon,
        value: anchor,
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
        value: "[" + surface_points.len().to_string() + "]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "points",
        type_name: "point3f[]",
        value: points_literal(encoded_points),
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "widths",
        type_name: "float[]",
        value: "[" + route_ribbon_width().to_string() + "]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@runtime@",
        path: geometry,
        name: "normals",
        type_name: "normal3f[]",
        value: ribbon_normals_literal(encoded_normals),
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
    let read = route_positions(context.doc_id, context.route_path);
    if read.ok != true { return read; }
    let ops = ribbon_ops(context.doc_id, context.route_path, read.points);
    apply_view_ops(context.doc_id, "Update route ribbon", ops)
}

fn marker_ops(doc, route, position) {
    let name = next_point_name(doc, route);
    if name == () {
        return #{ ok: false,
            error: "route stage is not synchronized; retry after program selection settles" };
    }
    let path = route + "/" + name;
    let zone = "route_" + name;
    let target = target_record(doc, route, "@root@");
    if !target.ok || !target.exists {
        return #{ ok: false, error: "selected route is not present in the composed USD stage" };
    }
    let edit_target = if target.under_arc { "@runtime@" } else { "@root@" };
    #{ ok: true, path: path, ops: [
        #{ AddPrim: #{
            edit_target: edit_target,
            parent_path: route,
            name: name,
            type_name: "Xform",
            reference: "lunco://markers/route_point.usda",
            reference_prim_path: "",
        }},
        #{ SetTranslate: #{
            edit_target: edit_target,
            path: path,
            value: position,
        }},
        #{ SetAttribute: #{
            edit_target: edit_target,
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
    let pointer = pointer_point(context);
    if pointer.ok != true { return failure(pointer.error); }
    let route = route_edit_path(context);
    if route == () {
        return failure("select an authored route program before adding a point");
    }
    let point = marker_ops(context.doc_id, route, point_values(pointer.point));
    if !point.ok { return failure(point.error); }
    let result = apply_ops(context.doc_id, "Append route point", point.ops);
    if result.ok { result.path = point.path; }
    result
}

fn on_move(context) {
    if context == () || context.doc_id == () {
        return failure("move has no active document");
    }
    let pointer = pointer_point(context);
    if pointer.ok != true { return failure(pointer.error); }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("move target is not a direct route point"); }
    let target = target_record(context.doc_id, point, "@root@");
    if !target.ok || !target.exists { return failure("move target does not exist"); }
    let edit_target = if target.authored_here && !target.under_arc { "@root@" } else { "@runtime@" };
    let ops = [#{ SetTranslate: #{
            edit_target: edit_target,
            path: point,
            value: point_values(pointer.point),
        }}];
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

    let op = if target.under_arc {
        // A referenced child cannot be removed from the referencing layer. USD
        // expresses this edit as a stronger local active=false opinion, even
        // when the local layer already carries transform or other overrides.
        #{ SetActive: #{ edit_target: "@runtime@", path: point, active: false } }
    } else if target.authored_here {
        #{ RemovePrim: #{ edit_target: "@root@", path: point } }
    } else {
        let runtime = target_record(context.doc_id, point, "@runtime@");
        if runtime.ok && runtime.authored_here {
            #{ RemovePrim: #{ edit_target: "@runtime@", path: point } }
        } else {
            return failure("route point is composed read-only");
        }
    };
    let ops = [op];
    let result = apply_ops(context.doc_id, "Delete route point", ops);
    if result.ok { result.path = point; }
    result
}

// Select the canonical live projection for a route point. The route point
// remains an ordinary USD prim; the standard gizmo can then operate on it.
fn on_select(context) {
    if context == () || context.doc_id == () {
        return failure("selection has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("selection target is not a direct route point"); }
    // Unattended scene tests and server runs intentionally have no editor
    // selection owner. The route context is still valid there; only an
    // attended editor has a selection side effect to perform.
    if is_unattended() { return #{ ok: true, path: point, selected: false }; }
    // The generic pointer target may be a nested visual, or a lower object when
    // the marker passes through primary picking. Once the policy has resolved
    // the exact authored route-point path, resolve that canonical identity
    // directly instead of trusting the semantic target chosen for selection.
    let entity_id = find_path(point);
    // Authoring and live projection are separate revisions. A pointer event
    // can arrive after the document edit but before the entity projection has
    // spawned; keep the route target valid and let the caller continue with a
    // pending selection instead of surfacing a transient edit error.
    if entity_id < 0 {
        return #{ ok: true, path: point, selected: false, pending: true };
    }
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

// Secondary pointer context opens a generic authored menu. The editor owns
// only popup placement and dispatch; labels and actions are authored here.
fn on_context(context) {
    if context == () || context.doc_id == () {
        return failure("context click has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if point == () { return failure("context target is not a direct route point"); }
    if is_unattended() { return #{ ok: true, path: point, selected: false }; }
    let selected = on_select(context);
    if selected.ok != true { return selected; }
    if context.screen_position == () {
        return failure("context click has no screen position");
    }
    let opened = open_context_menu(context.screen_position, [
        #{
            label: "Select route point",
            tool: "waypoint_editor",
            hook: "on_select",
            args: context,
        },
        #{
            label: "Delete route point",
            tool: "waypoint_editor",
            hook: "on_delete",
            args: context,
        },
    ]);
    if !opened { return failure("the scene context-menu host is unavailable"); }
    #{ ok: true, path: point, menu: true }
}
