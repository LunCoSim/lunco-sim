// Authored route editing policy.
//
// Rust supplies only the generic pointer context and typed USD-operation
// command. This policy creates ordinary USD prims and a disposable route-view
// overlay; no route, waypoint, or vehicle type is known by the engine.

// Module functions are used instead of top-level variables because imported
// Rhai tool functions execute in their own module scope.
// Keep the overview ribbon readable above the terrain renderer's LOD seams.
fn route_ribbon_width() { 0.12 }
fn route_ribbon_clearance() { 0.03 }
// Keep the transient BasisCurves payload below the Rhai/USD operation limit
// while still sampling more densely than the original long-leg chord
// approximation.
fn route_ribbon_sample_spacing() { 3.0 }
// A transient USD attribute is lowered through a bounded string payload. Keep
// the whole route below that transport limit; long legs increase their sample
// spacing adaptively while every authored waypoint remains an endpoint. This
// is deliberately bounded below the input-frame budget: route authoring must
// never synchronously materialise a large presentation payload.
fn route_ribbon_max_samples() { 256 }

// Marker appearance is a generic route-tool policy. State is a transient view
// of the authored program's visited set, written through the standard gprim
// displayColor attribute; the marker asset supplies only the initial visual.
fn unvisited_marker_color() { [0.10, 0.95, 0.22] }
fn visited_marker_color() { [0.55, 0.55, 0.55] }

fn ui_label() { "Route point" }
fn ui_hint() { "Add points to the active route's Twin Runtime layer; they persist when runtime persistence is enabled" }

// Called by the one scene-interaction policy for an unarmed pointer gesture.
// Physical button/modifier meaning is resolved by shared input settings; this
// policy consumes only semantic pointer intents.
fn route_pointer_action(context) {
    if context == () { return #{ ok: true, changed: false, handled: false }; }
    if pointer_intent(context, "route.add_point") {
        let point = pointer_surface_point(context);
        // Add-point is a terrain authoring gesture. A click into the sky is
        // deliberately consumed as a no-op: it must not create a notification
        // storm or reinterpret an arbitrary mesh/pointer position as terrain.
        if point.ok != true {
            return #{ ok: true, changed: false, handled: true, reason: "terrain_required" };
        }
        // Every semantic pointer owner must explicitly claim the gesture.
        // The edit result carries the created path, while `handled` tells the
        // central router to stop before selection or gizmo policies run.
        let result = on_click(context);
        result.handled = true;
        return result;
    }
    let policy = context["pointer_policy"];
    let side = if context.button == "primary" { "left" }
        else if context.button == "secondary" { "right" }
        else { "" };
    if pointer_intent(context, "route.context")
        && side != "" && policy != () && policy[side] == "context" {
        let route = route_edit_path(context);
        let point = context_route_point(context, route);
        let ribbon = context_route_ribbon(context, route);
        if route == () || (point == () && !ribbon) {
            return #{ ok: true, changed: false, handled: false };
        }
        let result = on_context(context);
        result.handled = true;
        return result;
    }
    // An armed route-point move is committed by the next ordinary scene hit.
    // This stays ahead of generic selection and possession routing, while
    // explicit Alt-click append keeps its existing meaning above.
    if context.button == "primary" {
        let move = move_selected_from_pointer(context);
        if move != () && move.handled == true { return move; }
        let route = route_edit_path(context);
        if route != () && context.doc_id != ()
            && direct_route_point(context.doc_id, context.selected_path, route) {
            clear_move_preview(context.doc_id, route);
        }
    }
    #{ ok: true, changed: false, handled: false }
}

fn move_selected_from_pointer(context) {
    if context == () || context.doc_id == () || context.selected_path == () {
        return ();
    }
    let route = route_edit_path(context);
    if route == () || !direct_route_point(context.doc_id, context.selected_path, route) {
        return ();
    }
    if !move_is_armed(context.doc_id, route, context.selected_path) {
        return ();
    }
    // Keep marker-to-marker clicks available for their own context/selection
    // semantics. Placement is for a world surface or another scene surface.
    if context_route_point(context, route) != () { return (); }
    let pointer = pointer_point(context);
    if pointer.ok != true {
        return #{ ok: true, changed: false, handled: false };
    }
    let result = move_route_point(context.doc_id, route, context.selected_path,
        point_values(pointer.point));
    result.handled = true;
    result
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
    if doc == () { return (); }
    usd_document_generation(doc)
}

fn prim_exists(doc, path) {
    // Runtime presentation reads follow the composed stage. The authored
    // document can be one journal revision ahead while a transient view edit
    // is being projected, and that must not make the ribbon disappear.
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false
}

// Route points are identified by the standard trigger-zone child authored by
// the route plan. Names and disposable presentation prims are not topology.
fn is_route_point(doc, path) {
    if doc == () || path == () || path == "" { return false; }
    // Point identity is resolved from the canonical authoring boundary. The
    // live projection may be one generation behind after a move or add, so a
    // render query cannot be used to decide whether the trigger child exists.
    let authored_trigger = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path + "/Trigger",
        edit_target: "@root@",
    });
    let resolved = authored_trigger != () && authored_trigger.ok != false;
    if !resolved { return false; }
    if authored_trigger.composed_exists == true { return true; }
    if authored_trigger.authored_in_document == true { return true; }
    false
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
        // Presentation is disposable @view@ state. A projection-generation
        // race is retried by its owner and is never reported as a failed
        // durable authoring command.
        return #{ ok: false, retryable: true,
            error: if result == () { "USD view edit returned no result" } else {
                result.error
            } };
    }
    #{ ok: true, changed: true }
}

// Synchronize all route-point marker colours from the policy-owned visited
// set. The map is keyed by canonical USD point path, so adding a point starts
// it green and editing a route cannot accidentally transfer state by index.
// This is called only at a document/view revision boundary by the route
// program; it is not a per-frame render update.
fn sync_marker_states(doc, route, visited) {
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
    for point in points.children {
        if !is_route_point(doc, point) { continue; }
        let marked = if visited == () { () } else { visited[point] };
        let color = if marked == true { visited_marker_color() }
            else { unvisited_marker_color() };
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
                edit_target: "@view@",
                path: point + "/Dome",
                name: "primvars:displayColor",
                type_name: "color3f[]",
                value: "[" + point_literal(color) + "]",
            }});
        }
    }
    apply_view_ops(doc, "Update route-point states", ops)
}

fn route_edit_path(context) {
    let explicit = context["route_path"];
    if explicit != () && explicit != "" {
        // Context-menu actions carry the exact point and route resolved by the
        // original pointer event. Validate that pairing against the canonical
        // document so the later menu callback does not need to rediscover a
        // viewport query scope that no longer exists.
        if context_route_point(context, explicit) != () {
            return explicit;
        }
        // Callers may carry a selected program identity rather than its
        // containing route scope. Normalize both forms at this policy
        // boundary so a program prim can never become an AddPrim parent.
        let route = canonical_route_path(
            context["doc_id"], explicit, subject_path(context),
        );
        if route != () { return route; }
        return canonical_route_path(context["doc_id"], explicit, ());
    }
    let doc = context["doc_id"];
    let subject = subject_path(context);
    if doc == () { return (); }

    // A pointer target is the route identity for this gesture. A possessed
    // rover can remain selected from an earlier edit, so resolving selection
    // first can route a click on another program's waypoint to the wrong route.
    let pointer_targets = [
        context["target_path"],
        context["hit_path"],
    ];
    for path in pointer_targets {
        let route = route_for_path(doc, path, subject);
        if route != () { return route; }
        let pointer_route = route_for_path(doc, path, ());
        if pointer_route != () { return pointer_route; }
    }
    if context["prim_paths"] != () {
        for path in context["prim_paths"] {
            let route = route_for_path(doc, path, subject);
            if route != () { return route; }
            let pointer_route = route_for_path(doc, path, ());
            if pointer_route != () { return pointer_route; }
        }
    }

    // With no route-bearing pointer target, an explicitly selected program or
    // waypoint still chooses the route for terrain add and other selection-led
    // edits. A selected program is the unambiguous route identity for the
    // dynamic program HUD when several programs control the same subject.
    let selected = context["selected_path"];
    let selected_route = route_for_path(doc, selected, subject);
    if selected_route != () { return selected_route; }
    let selected_point_route = route_for_path(doc, selected, ());
    if selected_point_route != () { return selected_point_route; }
    if selected != () && is_route_program(doc, selected, subject) {
        let program_route = route_parent_path(selected);
        if program_route != () { return program_route; }
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
        if is_route_scope(doc, current, subject) {
            return current;
        }
        if current == "/" { break; }
        current = route_parent_path(current);
        if current == () { break; }
    }
    ()
}

fn is_route_scope(doc, path, subject) {
    if path == () || path == "" { return false; }
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path, children: true });
    if result == () || result.ok == false || result.children == () {
        return false;
    }
    for child in result.children {
        if is_route_program(doc, child, subject) { return true; }
    }
    false
}

fn canonical_route_path(doc, path, subject) {
    if doc == () || path == () || path == "" { return (); }
    if is_route_program(doc, path, subject) {
        return route_parent_path(path);
    }
    if is_route_scope(doc, path, subject) { return path; }
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

fn direct_route_point(doc, path, route) {
    if path == () || path == "" || route == () || route == "" {
        return false;
    }
    let prefix = route + "/";
    if !path.starts_with(prefix) { return false; }
    let remainder = path.sub_string(prefix.len());
    let direct = remainder != "" && remainder.index_of("/") < 0;
    if !direct { return false; }
    // Keep the edit target check in this same scope as the direct-child check.
    // A freshly moved point may be ahead of the projected scene, but its
    // canonical trigger target is already committed and remains authoritative.
    let trigger = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path + "/Trigger",
        edit_target: "@root@",
    });
    if trigger == () || trigger.ok == false { return false; }
    if trigger.composed_exists == true { return true; }
    trigger.authored_in_document == true
}

fn context_route_point(context, route) {
    if context == () { return (); }
    let doc = context["doc_id"];
    if doc == () { return (); }
    let candidates = [];
    if context.target_path != () { candidates.push(context.target_path); }
    if context.hit_path != () { candidates.push(context.hit_path); }
    if context.prim_paths != () {
        for path in context.prim_paths { candidates.push(path); }
    }
    for path in candidates {
        let direct = direct_route_point(doc, path, route);
        if direct { return path; }
    }
    ()
}

fn context_route_ribbon(context, route) {
    if context == () || route == () { return false; }
    let ribbon = ribbon_geometry_path(route);
    let candidates = [];
    if context.target_path != () { candidates.push(context.target_path); }
    if context.hit_path != () { candidates.push(context.hit_path); }
    if context.prim_paths != () {
        for path in context.prim_paths { candidates.push(path); }
    }
    for path in candidates {
        if path == ribbon || path.starts_with(ribbon + "/") { return true; }
    }
    false
}

fn delete_route_point(context, route) {
    let point = context_route_point(context, route);
    if point != () { return point; }
    if context == () || context["doc_id"] == () { return (); }

    // Selection-only delete commands have no pointer target to resolve. Use
    // the selected point only when no pointer paths were supplied, so an
    // unrelated scene selection cannot override a real click target.
    let has_pointer_target = context["target_path"] != ()
        || context["hit_path"] != ()
        || (context["prim_paths"] != () && context["prim_paths"].len() > 0);
    if has_pointer_target { return (); }
    let selected = context["selected_path"];
    if direct_route_point(context["doc_id"], selected, route) {
        return selected;
    }
    ()
}

fn next_point_name(doc, route, read) {
    if read == () || read.ok != true || read.children == () { return (); }
    let prefix = "P";
    for child in read.paths {
        let parts = child.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf.starts_with("W") { prefix = "W"; break; }
    }
    let index = 0;
    while index < 100000 {
        let candidate = prefix + index.to_string();
        let path = route + "/" + candidate;
        let used = read.children.contains(path);
        if !used {
            // Composed child enumeration omits inactive prims. Keep their
            // names reserved as well: reusing one would inherit its inactive
            // opinion, so the subsequent primOrder would name a non-child.
            let target = target_record(doc, path, "@runtime@");
            if !target.ok { return (); }
            if !target.exists { return candidate; }
        }
        index += 1;
    }
    ()
}

// Read the route points from one settled composed-stage generation. The
// result feeds only the disposable ribbon view; authored route points remain
// the document's durable state. `paths` is the stable identity portion of the
// view key, so a transient ribbon revision cannot invalidate itself.
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
    let paths = [];
    if result == () || result.ok == false || result.children == () {
        return #{ ok: false, retryable: true, error: "route stage is not synchronized" };
    }
    for child in result.children {
        if !is_route_point(doc, child) { continue; }
        let record = query("QueryUsdPrim", #{ doc_id: doc, path: child });
        if record == () || record.ok == false {
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
        paths.push(child);
    }
    #{ ok: true, paths: paths, points: positions, children: result.children }
}

// A route view depends on authored point identity and placement, not on the
// document generation used by disposable @view@ presentation edits. Keep the
// key at the policy boundary so the generic live projector does not need to
// know that a particular consumer is a route ribbon.
fn route_view_key(read) {
    if read == () || read.paths == () || read.points == ()
        || read.paths.len() != read.points.len() {
        return ();
    }
    let key = "";
    for i in 0..read.paths.len() {
        let point = read.points[i];
        key += read.paths[i] + ":" + point[0].to_string() + ","
            + point[1].to_string() + "," + point[2].to_string() + ";";
    }
    key
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
        return #{ ok: true, points: sampled_points, normals: sampled_normals };
    }
    let dense_points = [];
    if points.len() == 1 {
        dense_points.push(points[0]);
    } else {
        let spacing = ribbon_sampling_spacing(points);
        for i in 0..(points.len() - 1) {
            let a = points[i];
            let b = points[i + 1];
            let steps = ribbon_step_count(a, b, spacing);
            for j in 0..steps {
                let t = j.to_float() / steps.to_float();
                dense_points.push([
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ]);
            }
        }
        dense_points.push(points[points.len() - 1]);
    }
    let coordinates = [];
    for point in dense_points { coordinates.push([point[0], point[2]]); }
    let heights = query("TerrainHeights", #{ points: coordinates, eps: 1.0 });
    if heights == () || heights.len() != dense_points.len() {
        return #{ ok: false, retryable: true,
            error: "batched terrain sampling did not return one result per ribbon point" };
    }
    // A scene without an analytic terrain provider authors its route points as
    // the available support geometry. The query still returns an ordered,
    // explicit `found: false` result for those samples.
    if heights[0].found != true {
        for point in points {
            sampled_points.push(point);
            sampled_normals.push([0.0, 1.0, 0.0]);
        }
        return #{ ok: true, points: sampled_points, normals: sampled_normals };
    }
    for i in 0..dense_points.len() {
        let point = dense_points[i];
        let sample = heights[i];
        if sample.found == true && sample.height != () && sample.normal != () {
            let normal = vnorm(sample.normal);
            if normal != () {
                sampled_points.push([point[0], sample.height, point[2]]);
                sampled_normals.push(normal);
                continue;
            }
        }
        sampled_points.push(point);
        sampled_normals.push([0.0, 1.0, 0.0]);
    }
    #{ ok: true, points: sampled_points, normals: sampled_normals }
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
        if a[i] == () || b[i] == () || a[i].len() != b[i].len() { return false; }
        for axis in 0..a[i].len() {
            let magnitude = a[i][axis].abs();
            if b[i][axis].abs() > magnitude { magnitude = b[i][axis].abs(); }
            // USD point3f readback is narrowed to f32 at the schema boundary.
            // Compare within half an f32 ULP so a stable point does not cause
            // every presentation refresh to rewrite the whole ribbon.
            let tolerance = magnitude * 0.000000059604644775390625;
            if (a[i][axis] - b[i][axis]).abs() > tolerance.max(0.000001) {
                return false;
            }
        }
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
        schemas: true,
        attrs: ["type", "wrap", "curveVertexCounts", "points", "widths", "normals",
            "lunco:interaction:left", "lunco:interaction:right"],
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
        || !points_equal(shape.attrs["normals"], local_normals)
        || shape.api_schemas == ()
        || !shape.api_schemas.contains("LunCoPointerInteractionAPI")
        || shape.attrs["lunco:interaction:left"] != "pass_through"
        || shape.attrs["lunco:interaction:right"] != "context" {
        return true;
    }
    false
}

fn ribbon_interaction_ops(doc, geometry, geometry_exists) {
    let api_schemas = ["MaterialBindingAPI", "LunCoSurfaceAPI"];
    let left = ();
    let right = ();
    if geometry_exists {
        let shape = query("QueryUsdPrim", #{
            doc_id: doc,
            path: geometry,
            attrs: ["lunco:interaction:left", "lunco:interaction:right"],
            schemas: true,
        });
        if shape == () || shape.ok == false || shape.attrs == ()
            || shape.api_schemas == () {
            return ();
        }
        api_schemas = shape.api_schemas;
        left = shape.attrs["lunco:interaction:left"];
        right = shape.attrs["lunco:interaction:right"];
    }
    let ops = [];
    if !api_schemas.contains("LunCoPointerInteractionAPI") {
        api_schemas.push("LunCoPointerInteractionAPI");
        ops.push(#{ SetApiSchemas: #{
            edit_target: "@view@",
            path: geometry,
            schemas: api_schemas,
        }});
    }
    if left != "pass_through" {
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: geometry,
            name: "lunco:interaction:left",
            type_name: "token",
            value: "\"pass_through\"",
        }});
    }
    if right != "context" {
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: geometry,
            name: "lunco:interaction:right",
            type_name: "token",
            value: "\"context\"",
        }});
    }
    ops
}

fn ribbon_ops(doc, route, points) {
    let ribbon = ribbon_path(route);
    if points.len() < 2 {
        if prim_exists(doc, ribbon) {
            return #{ ok: true, ops: [#{ RemovePrim: #{
                edit_target: "@view@",
                path: ribbon,
            }}] };
        }
        return #{ ok: true, ops: [] };
    }
    let sampled = ribbon_surface_samples(points);
    if sampled.ok != true { return sampled; }
    let surface_points = sampled.points;
    let surface_normals = sampled.normals;
    if surface_points.len() < 2 || surface_normals.len() != surface_points.len() {
        return #{ ok: true, ops: [] };
    }
    let geometry = ribbon_geometry_path(route);
    let geometry_exists = prim_exists(doc, geometry);
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
        return #{ ok: true, ops: [] };
    }

    let ops = [];
    if !prim_exists(doc, ribbon) {
        let parts = ribbon.split("/");
        let parent = "";
        for i in 1..parts.len() - 1 { parent += "/" + parts[i]; }
        ops.push(#{ AddPrim: #{
            edit_target: "@view@",
            parent_path: parent,
            name: parts[parts.len() - 1],
            type_name: "Xform",
            reference: "lunco://markers/route_ribbon.usda",
            reference_prim_path: "",
        }});
    }
    ops.push(#{ SetTranslate: #{
        edit_target: "@view@",
        path: ribbon,
        value: anchor,
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "type",
        type_name: "token",
        value: "\"linear\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "wrap",
        type_name: "token",
        value: "\"nonperiodic\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "curveVertexCounts",
        type_name: "int[]",
        value: "[" + surface_points.len().to_string() + "]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "points",
        type_name: "point3f[]",
        value: points_literal(encoded_points),
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "widths",
        type_name: "float[]",
        value: "[" + route_ribbon_width().to_string() + "]",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: "@view@",
        path: geometry,
        name: "normals",
        type_name: "normal3f[]",
        value: ribbon_normals_literal(encoded_normals),
    }});
    let interaction_ops = ribbon_interaction_ops(doc, geometry, geometry_exists);
    if interaction_ops == () {
        return #{ ok: false, retryable: true,
            error: "route ribbon pointer policy is not synchronized" };
    }
    for op in interaction_ops { ops.push(op); }
    #{ ok: true, ops: ops }
}

// Route points are authored facts. The ribbon is a disposable tool view in
// the @view@ layer, so a Twin remains portable and contains no persisted
// presentation prim.
fn ensure_ribbon(context) {
    if context == () || context.doc_id == () || context.route_path == () {
        return failure("route view has no explicit document or route path");
    }
    let route = canonical_route_path(context.doc_id, context.route_path, ());
    if route == () {
        return failure("route view path is not an authored program scope");
    }
    let read = route_positions(context.doc_id, route);
    if read.ok != true { return read; }
    let plan = ribbon_ops(context.doc_id, route, read.points);
    if plan.ok != true { return plan; }
    apply_view_ops(context.doc_id, "Update route ribbon", plan.ops)
}

fn marker_ops(doc, route, position, read) {
    let route = canonical_route_path(doc, route, ());
    if route == () {
        return #{ ok: false,
            error: "selected path is not an authored route program scope" };
    }
    let name = next_point_name(doc, route, read);
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
    #{ ok: true, path: path, ops: [
        #{ AddPrim: #{
            edit_target: "@runtime@",
            parent_path: route,
            name: name,
            type_name: "Xform",
            reference: "lunco://markers/route_point.usda",
            reference_prim_path: "",
        }},
        #{ SetTranslate: #{
            edit_target: "@runtime@",
            path: path,
            value: position,
        }},
        #{ SetAttribute: #{
            edit_target: "@runtime@",
            path: path + "/Trigger",
            name: "lunco:triggerZone",
            type_name: "token",
            value: "\"" + zone + "\"",
        }},
    ] }
}

fn move_route_point(doc, route, point, position) {
    if doc == () || route == () || point == () || position == () {
        return failure("move has an incomplete route target or position");
    }
    if !direct_route_point(doc, point, route) {
        return failure("move target is not a direct route point");
    }
    let target = target_record(doc, point, "@runtime@");
    if !target.ok || !target.exists {
        return failure("move target does not exist");
    }
    // Pointer positions are in the active physics frame. MoveEntity is the
    // shared pose owner that converts them into this prim's parent-local USD
    // transform and persists the same live move to Runtime.
    let entity_id = find_path(point);
    if entity_id < 0 {
        return #{ ok: false, retryable: true,
            error: "move target has no live scene projection" };
    }
    let result = cmd("MoveEntity", #{ entity_id: entity_id, translation: position });
    if result == () || result.ok != true {
        return failure(if result == () { "MoveEntity returned no result" } else { result.error });
    }
    result.path = point;
    clear_move_preview(doc, route);
    result
}

fn route_endpoint_extension_distance() { 5.0 }

fn insertion_position(read, target, before) {
    if read == () || read.ok != true || read.paths.len() < 2 { return (); }
    let index = -1;
    for i in 0..read.paths.len() {
        if read.paths[i] == target { index = i; break; }
    }
    if index < 0 { return (); }
    let here = read.points[index];
    if before && index > 0 {
        let adjacent = read.points[index - 1];
        return [
            (adjacent[0] + here[0]) * 0.5,
            (adjacent[1] + here[1]) * 0.5,
            (adjacent[2] + here[2]) * 0.5,
        ];
    }
    if !before && index + 1 < read.paths.len() {
        let adjacent = read.points[index + 1];
        return [
            (here[0] + adjacent[0]) * 0.5,
            (here[1] + adjacent[1]) * 0.5,
            (here[2] + adjacent[2]) * 0.5,
        ];
    }
    let adjacent = if before { read.points[1] }
        else { read.points[read.points.len() - 2] };
    let direction = [
        here[0] - adjacent[0],
        here[1] - adjacent[1],
        here[2] - adjacent[2],
    ];
    let length = vlen(direction);
    if length == () || length <= 1e-6 { return (); }
    let step = route_endpoint_extension_distance();
    [
        here[0] + direction[0] * step / length,
        here[1] + direction[1] * step / length,
        here[2] + direction[2] * step / length,
    ]
}

fn insertion_action_label(read, target, before) {
    if read == () || read.ok != true { return (); }
    let index = -1;
    for i in 0..read.paths.len() {
        if read.paths[i] == target { index = i; break; }
    }
    if index < 0 { return (); }
    let current = child_name(target);
    let extension = " (+" + route_endpoint_extension_distance().to_string() + " m)";
    if before && index == 0 {
        return "Add point before " + current + extension;
    }
    if !before && index + 1 == read.paths.len() {
        return "Add point after " + current + extension;
    }
    let first = if before { child_name(read.paths[index - 1]) } else { current };
    let second = if before { current } else { child_name(read.paths[index + 1]) };
    "Add point between " + first + " and " + second + " (midpoint)"
}

fn child_name(path) {
    let parts = path.split("/");
    parts[parts.len() - 1]
}

fn insert_route_point(context, before) {
    if context == () || context.doc_id == () {
        return failure("insert has no active document");
    }
    let route = route_edit_path(context);
    let target = context_route_point(context, route);
    if route == () || target == () {
        return failure("insert target is not a direct route point");
    }
    // Use one document-scoped composed read for point placement, name
    // allocation, and order. The order includes only active composed children;
    // name allocation also reserves authored inactive children.
    let read = route_positions(context.doc_id, route);
    if read.ok != true || read.paths.len() < 2 {
        return failure("before/after insertion needs an adjacent active route point");
    }
    let position = insertion_position(read, target, before);
    if position == () {
        return failure("insert target is active but its endpoint direction cannot be inferred");
    }
    let marker = marker_ops(context.doc_id, route, position, read);
    if marker.ok != true { return failure(marker.error); }
    let name = child_name(marker.path);
    let order = [];
    let inserted = false;
    for child in read.paths {
        if child == target && before && !inserted {
            order.push(name);
            inserted = true;
        }
        order.push(child_name(child));
        if child == target && !before && !inserted {
            order.push(name);
            inserted = true;
        }
    }
    if !inserted { return failure("insert target is no longer a direct route child"); }
    marker.ops.push(#{ SetPrimOrder: #{
        edit_target: "@runtime@",
        path: route,
        order: order,
    }});
    let result = apply_ops(context.doc_id,
        if before { "Insert route point before" } else { "Insert route point after" },
        marker.ops);
    if result.ok { result.path = marker.path; }
    result
}

fn on_insert_before(context) { insert_route_point(context, true) }
fn on_insert_after(context) { insert_route_point(context, false) }

fn preview_path(route) { route + "/MovePreview" }

fn move_target_path(route, point) {
    preview_path(route) + "/Target_" + child_name(point)
}

fn move_is_armed(doc, route, point) {
    let target = target_record(doc, move_target_path(route, point), "@view@");
    target.ok && target.authored_here
}

fn clear_move_preview(doc, route) {
    let path = preview_path(route);
    let target = target_record(doc, path, "@view@");
    // A missing preview is the idle state; clearing it is idempotent.
    if !target.ok || !target.exists {
        return #{ ok: true, changed: false };
    }
    apply_view_ops(doc, "Clear route-point move preview", [#{ RemovePrim: #{
        edit_target: "@view@",
        path: path,
    }}])
}

fn move_preview_ops(doc, route, point, position) {
    let path = preview_path(route);
    let target = target_record(doc, path, "@view@");
    let ops = [];
    if !target.ok || !target.exists {
        let marker = query("QueryUsdPrim", #{
            doc_id: doc,
            path: point + "/Dome",
            attrs: ["radius", "xformOp:translate"],
        });
        if marker == () || marker.ok == false || marker.attrs == ()
            || marker.attrs["radius"] == () || marker.attrs["xformOp:translate"] == () {
            return #{ ok: false, retryable: true,
                error: "selected route point marker geometry is not synchronized" };
        }
        ops.push(#{ AddPrim: #{
            edit_target: "@view@",
            parent_path: route,
            name: "MovePreview",
            type_name: "Xform",
            reference: (),
            reference_prim_path: "",
        }});
        ops.push(#{ SetApiSchemas: #{
            edit_target: "@view@",
            path: path,
            schemas: ["LunCoPointerInteractionAPI"],
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: path,
            name: "lunco:interaction:left",
            type_name: "token",
            value: "\"pass_through\"",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: path,
            name: "lunco:interaction:right",
            type_name: "token",
            value: "\"pass_through\"",
        }});
        ops.push(#{ AddPrim: #{
            edit_target: "@view@",
            parent_path: path,
            name: "Dome",
            type_name: "Sphere",
            reference: (),
            reference_prim_path: "",
        }});
        let dome = path + "/Dome";
        ops.push(#{ SetApiSchemas: #{
            edit_target: "@view@",
            path: dome,
            schemas: ["LunCoSurfaceAPI", "LunCoPointerInteractionAPI"],
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "lunco:interaction:left",
            type_name: "token",
            value: "\"pass_through\"",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "lunco:interaction:right",
            type_name: "token",
            value: "\"pass_through\"",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "radius",
            type_name: "double",
            value: marker.attrs["radius"].to_string(),
        }});
        ops.push(#{ SetTranslate: #{
            edit_target: "@view@",
            path: dome,
            value: marker.attrs["xformOp:translate"],
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "primvars:displayColor",
            type_name: "color3f[]",
            value: "[(0.05, 0.75, 1.0)]",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "primvars:displayOpacity",
            type_name: "float[]",
            value: "[0.72]",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "lunco:surface:unlit",
            type_name: "bool",
            value: "true",
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: "@view@",
            path: dome,
            name: "primvars:doNotCastShadows",
            type_name: "bool",
            value: "true",
        }});
        ops.push(#{ AddPrim: #{
            edit_target: "@view@",
            parent_path: path,
            name: "Target_" + child_name(point),
            type_name: "Xform",
            reference: (),
            reference_prim_path: "",
        }});
    }
    ops.push(#{ SetTranslate: #{
            edit_target: "@view@",
            path: path,
            value: position,
        }});
    #{ ok: true, ops: ops }
}

fn on_pointer_move(context) {
    if context == () || context.doc_id == () || context.selected_path == () {
        return #{ ok: true, changed: false };
    }
    let route = route_edit_path(context);
    if route == () || !direct_route_point(context.doc_id, context.selected_path, route) {
        return #{ ok: true, changed: false };
    }
    if !move_is_armed(context.doc_id, route, context.selected_path) {
        return #{ ok: true, changed: false };
    }
    let pointer = pointer_point(context);
    if pointer.ok != true { return #{ ok: true, changed: false }; }
    let preview = find_path(preview_path(route));
    if preview < 0 {
        return #{ ok: false, retryable: true,
            error: "route-point move preview is not projected yet" };
    }
    let result = cmd("SetUsdViewPreviewTransform", #{
        doc_id: context.doc_id,
        entity_id: preview,
        translation: point_values(pointer.point),
    });
    if result == () || result.ok != true {
        return #{ ok: false, retryable: true,
            error: if result == () { "route-point preview transform returned no result" }
                else { result.error } };
    }
    #{ ok: true, changed: true }
}

fn on_add_here(context) {
    if context == () || context.doc_id == () {
        return failure("ribbon add has no active document");
    }
    let route = route_edit_path(context);
    if route == () { return failure("ribbon is not part of an authored route"); }
    let pointer = pointer_point(context);
    if pointer.ok != true { return failure(pointer.error); }
    let position = point_values(pointer.point);
    let heights = query("TerrainHeights", #{ points: [[position[0], position[2]]], eps: 1.0 });
    if heights != () && heights.len() == 1 && heights[0].found == true
        && heights[0].height != () {
        position[1] = heights[0].height;
    }
    let read = route_positions(context.doc_id, route);
    if read.ok != true { return failure("route stage is not synchronized"); }
    let marker = marker_ops(context.doc_id, route, position, read);
    if marker.ok != true { return failure(marker.error); }
    let result = apply_ops(context.doc_id, "Add route point on ribbon", marker.ops);
    if result.ok { result.path = marker.path; }
    result
}

fn on_click(context) {
    if context == () || context.button != "primary" {
        return #{ ok: true, changed: false, handled: false };
    }
    if context == () || context.doc_id == () || context.scene_root_path == () {
        return failure("click has no active document or scene root");
    }
    let pointer = pointer_surface_point(context);
    if pointer.ok != true { return failure(pointer.error); }
    let route = route_edit_path(context);
    if route == () {
        return failure("select an authored route program before adding a point");
    }
    let read = route_positions(context.doc_id, route);
    if read.ok != true { return failure("route stage is not synchronized"); }
    let point = marker_ops(context.doc_id, route, point_values(pointer.point), read);
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
    move_route_point(context.doc_id, route, point, point_values(pointer.point))
}

fn on_delete(context) {
    if context == () || context.doc_id == () {
        return failure("delete has no active document");
    }
    let route = route_edit_path(context);
    let point = delete_route_point(context, route);
    if point == () { return failure("delete target is not a direct route point"); }
    let target = target_record(context.doc_id, point, "@runtime@");
    if !target.ok || !target.exists { return failure("delete target does not exist"); }

    // A Runtime marker-color opinion can create a local spec at the waypoint
    // path without making Runtime its owner. The authored Trigger child is the
    // route-point creation fact: remove points created in Runtime, and use a
    // stronger active=false opinion for points owned by source/reference data.
    let runtime_trigger = target_record(
        context.doc_id, point + "/Trigger", "@runtime@",
    );
    if !runtime_trigger.ok {
        return failure("route-point ownership cannot be resolved in the Runtime layer");
    }
    let op = if runtime_trigger.authored_here {
        #{ RemovePrim: #{ edit_target: "@runtime@", path: point } }
    } else {
        #{ SetActive: #{ edit_target: "@runtime@", path: point, active: false } }
    };
    let result = apply_ops(context.doc_id, "Delete route point", [op]);
    if result.ok {
        result.path = point;
        clear_move_preview(context.doc_id, route);
    }
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
        inspector_part_entity_id: 0,
    });
    if result == () {
        return failure("SelectEntity command is unavailable in this host");
    }
    if result.ok == false {
        return failure(result.error);
    }
    #{ ok: true, path: point }
}

// The explicit Move menu action selects its point and creates the disposable
// ghost at that point's current route-local position. Its target marker is the
// policy state that arms subsequent hover and placement input.
fn on_move_begin(context) {
    if context == () || context.doc_id == () {
        return failure("move has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    if route == () || point == () {
        return failure("move target is not a direct route point");
    }
    let authored = query("QueryUsdPrim", #{
        doc_id: context.doc_id,
        path: point,
        attrs: ["xformOp:translate"],
    });
    if authored == () || authored.ok == false || authored.attrs == ()
        || authored.attrs["xformOp:translate"] == () {
        return failure("move target position is not synchronized");
    }
    let selected = on_select(context);
    if selected == () || selected.ok != true { return selected; }
    let cleared = clear_move_preview(context.doc_id, route);
    if cleared == () || cleared.ok != true {
        return failure("existing move preview could not be cleared");
    }
    let plan = move_preview_ops(context.doc_id, route, point,
        authored.attrs["xformOp:translate"]);
    if plan.ok != true { return failure(plan.error); }
    let result = apply_view_ops(context.doc_id, "Begin route-point move", plan.ops);
    if result.ok == true {
        result.path = point;
        result.armed = true;
    }
    result
}

// Secondary pointer context opens a generic authored menu without changing
// scene selection. Selection remains an explicit menu action, so opening the
// menu cannot also activate the transform gizmo.
fn on_context(context) {
    if context == () || context.doc_id == () {
        return failure("context click has no active document");
    }
    let route = route_edit_path(context);
    let point = context_route_point(context, route);
    let ribbon = point == () && context_route_ribbon(context, route);
    if point == () && !ribbon {
        return failure("context target is not a direct route point or its route ribbon");
    }
    if is_unattended() {
        let path = if ribbon { ribbon_geometry_path(route) } else { point };
        return #{ ok: true, path: path, selected: false };
    }
    if context.screen_position == () {
        return failure("context click has no screen position");
    }
    if ribbon {
        let action_context = #{
            doc_id: context["doc_id"],
            route_path: route,
            target_path: context["target_path"],
            hit_path: context["hit_path"],
            prim_paths: context["prim_paths"],
            world_position: context["world_position"],
        };
        let opened = open_context_menu(context.screen_position, [#{
            label: "Add point at cursor on route",
            tool: "waypoint_editor",
            hook: "on_add_here",
            args: action_context,
        }]);
        if !opened { return failure("the scene context-menu host is unavailable"); }
        return #{ ok: true, path: ribbon_geometry_path(route), menu: true, selected: false };
    }
    let action_context = #{
        doc_id: context["doc_id"],
        route_path: route,
        selected_path: point,
        target_path: point,
        hit_path: point,
        prim_paths: [point],
    };
    let items = [#{
            label: "Select route point",
            tool: "waypoint_editor",
            hook: "on_select",
            args: action_context,
        }];
    items.push(#{
        label: "Move point (click destination)",
        tool: "waypoint_editor",
        hook: "on_move_begin",
        args: action_context,
    });
    let route_read = route_positions(context.doc_id, route);
    if route_read.ok == true && route_read.paths.len() >= 2 {
        if insertion_position(route_read, point, true) != () {
            items.push(#{
                label: insertion_action_label(route_read, point, true),
                tool: "waypoint_editor",
                hook: "on_insert_before",
                args: action_context,
            });
        }
        if insertion_position(route_read, point, false) != () {
            items.push(#{
                label: insertion_action_label(route_read, point, false),
                tool: "waypoint_editor",
                hook: "on_insert_after",
                args: action_context,
            });
        }
    }
    items.push(#{
            label: "Delete route point",
            tool: "waypoint_editor",
            hook: "on_delete",
            args: action_context,
        });
    let opened = open_context_menu(context.screen_position, items);
    if !opened { return failure("the scene context-menu host is unavailable"); }
    #{ ok: true, path: point, menu: true, selected: false }
}
