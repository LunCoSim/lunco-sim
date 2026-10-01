// Authored route editing policy.
//
// Rust supplies generic pointer context, typed USD-operation commands, and a
// coalesced transient curve-view update. This policy owns route editing and
// view parameters; the engine knows no route, waypoint, or vehicle type.

// Module functions are used instead of top-level variables because imported
// Rhai tool functions execute in their own module scope.
// Terrain routes are sparse surface annotations; authored points remain the
// only navigation topology. Width is presentation policy, in metres.
fn route_ribbon_width() { 0.12 }

// Marker appearance is a generic route-tool policy. State is a transient view
// of the authored program's visited set, written through the standard gprim
// displayColor attribute; the marker asset supplies only the initial visual.
fn unvisited_marker_color() { [0.10, 0.95, 0.22] }
fn visited_marker_color() { [0.55, 0.55, 0.55] }

fn vector_equal(left, right) {
    if left == () || right == () || left.len() != right.len() { return false; }
    for i in 0..left.len() {
        let delta = left[i] - right[i];
        if delta > 1e-5 || delta < -1e-5 { return false; }
    }
    true
}

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
        let route = route_edit_path_with_scene_discovery(context, false);
        let point = context_route_point(context, route);
        let ribbon = context_route_ribbon(context, route);
        if route == () || (point == () && !ribbon) {
            return #{ ok: true, changed: false, handled: false };
        }
        let result = on_context(context);
        result.handled = true;
        return result;
    }
    // Only the registered interaction may intercept an ordinary primary hit.
    // Its document-scoped context supplies the route without scene discovery.
    if context.button == "primary" {
        let active = context.active_pointer_move;
        if active != () && active.tool == "scene_interaction"
            && active.hook == "on_pointer_move" && active.context != () {
            let route = active.context.route_path;
            if route != () && active.interaction_id == move_interaction_id(route) {
                let move = move_selected_from_pointer(context, route);
                if move != () && move.handled == true { return move; }
                let cleared = clear_move_preview(context.doc_id, route);
                if cleared.ok != true {
                    let result = failure(cleared.error);
                    result.handled = true;
                    return result;
                }
            }
        }
    }
    #{ ok: true, changed: false, handled: false }
}

fn move_selected_from_pointer(context, route) {
    if context == () || context.doc_id == () { return (); }
    let target = move_preview_target(context.doc_id, route);
    if target.ok != true || target.armed != true {
        let result = failure(if target.ok != true { target.error }
            else { "active route movement has no preview target" });
        result.handled = true;
        return result;
    }
    let point = target.point;
    // Marker hits retain their own context and selection semantics.
    if context_route_point(context, route) != () { return (); }
    let pointer = pointer_point(context);
    if pointer.ok != true {
        return #{ ok: true, changed: false, handled: false };
    }
    let result = move_route_point(context.doc_id, route, point,
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
    let snapshot = route_child_snapshot(doc, route, [], false);
    if snapshot.ok != true { return snapshot; }
    let points = [];
    for record in snapshot.records {
        if record.active == true && route_point_record(record) {
            points.push(record.path);
        }
    }
    if points.len() == 0 {
        return apply_view_ops(doc, "Update route-point states", []);
    }
    let marker_paths = [];
    for point in points { marker_paths.push(point + "/Dome"); }
    let markers = query("QueryUsdPrims", #{
        doc_id: doc,
        paths: marker_paths,
        attrs: ["primvars:displayColor"],
    });
    if markers == () || markers.ok == false || markers.records == ()
        || markers.records.len() != points.len() {
        return #{ ok: false, retryable: true,
            error: "route-point marker display state is not synchronized" };
    }
    let ops = [];
    for i in 0..points.len() {
        let point = points[i];
        let marked = if visited == () { () } else { visited[point] };
        let color = if marked == true { visited_marker_color() }
            else { unvisited_marker_color() };
        let marker = markers.records[i];
        if marker.path != marker_paths[i] || marker.attrs == ()
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
    route_edit_path_with_scene_discovery(context, true)
}

fn route_edit_path_with_scene_discovery(context, allow_scene_discovery) {
    if context == () { return (); }
    let explicit = context["route_path"];
    if type_of(explicit) == "string" && explicit != "" {
        // Context-menu actions carry the route and direct point from the
        // original pointer event. The consuming edit validates the point once;
        // this resolver checks only the path relationship to avoid repeating
        // a document query before every route action.
        if context_route_point_path(context, explicit) != () {
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
    let pointer_targets = [];
    for path in [context["target_path"], context["hit_path"]] {
        if type_of(path) == "string" && path != "" { pointer_targets.push(path); }
    }
    let prim_paths = context["prim_paths"];
    if type_of(prim_paths) == "array" {
        for path in prim_paths {
            if type_of(path) == "string" && path != "" { pointer_targets.push(path); }
        }
    }
    for path in pointer_targets {
        let route = route_for_path(doc, path, subject);
        if route != () { return route; }
        let pointer_route = route_for_path(doc, path, ());
        if pointer_route != () { return pointer_route; }
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
    let linked = linked_routes(
        doc, subject, context["scene_root_path"], allow_scene_discovery,
    );
    if linked.len() == 1 { return linked[0]; }
    if linked.len() > 1 { return (); }

    // A multi-program subject must first select one of those program prims
    // through the generic scene selection surface.
    ()
}

fn route_parent_path(path) {
    if type_of(path) != "string" || path == "" { return (); }
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
    if type_of(path) != "string" || path == "" { return false; }
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

fn record_is_route_program(record, subject) {
    if record == () || record.api_schemas == () || record.relationships == () {
        return false;
    }
    let is_program = false;
    for schema in record.api_schemas {
        if schema == "LunCoProgramAPI" { is_program = true; break; }
    }
    if !is_program { return false; }
    let targets = record.relationships["inputs:subject"];
    targets != () && targets.len() == 1
        && (subject == () || targets[0] == subject)
}

fn route_for_path(doc, path, subject) {
    if type_of(path) != "string" || path == "" { return (); }
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
    if type_of(path) != "string" || path == "" { return false; }
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path, children: true });
    if result == () || result.ok == false || result.children == () {
        return false;
    }
    if result.children.len() == 0 { return false; }
    let programs = query("QueryUsdPrims", #{
        doc_id: doc,
        paths: result.children,
        rels: ["inputs:subject"],
        schemas: true,
    });
    if programs == () || programs.ok == false || programs.records == () {
        return false;
    }
    for record in programs.records {
        if record_is_route_program(record, subject) { return true; }
    }
    false
}

fn canonical_route_path(doc, path, subject) {
    if doc == () || type_of(path) != "string" || path == "" { return (); }
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
        attrs: [],
        children: true,
    });
    if result == () || result.ok == false || result.children == () {
        return programs;
    }
    // Read one hierarchy level from a composed-stage snapshot instead of
    // resolving every prim through a separate QueryUsdPrim call.
    let frontier = result.children;
    for level in 0..(33 - depth) {
        if frontier.len() == 0 { break; }
        let batch = query("QueryUsdPrims", #{
            doc_id: doc,
            paths: frontier,
            attrs: [],
            rels: ["inputs:subject"],
            schemas: true,
            children: level < (32 - depth),
        });
        if batch == () || batch.ok == false || batch.records == ()
            || batch.records.len() != frontier.len() {
            return programs;
        }
        let nested = [];
        for record in batch.records {
            if record_is_route_program(record, subject) {
                programs.push(record.path);
            }
            if level < (32 - depth) && record.children != () {
                for child in record.children {
                    if !nested.contains(child) { nested.push(child); }
                }
            }
        }
        frontier = nested;
    }
    programs
}

fn linked_routes(doc, subject, scene_root, allow_scene_discovery) {
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
    if programs.len() == 0 && allow_scene_discovery {
        programs = discover_subject_programs(doc, scene_root, subject, 0);
    }
    for program in programs {
        let route = route_for_path(doc, program, subject);
        if route != () && !routes.contains(route) { routes.push(route); }
    }
    routes
}

fn subject_path(context) {
    let controlled = context["controlled_path"];
    if type_of(controlled) == "string" && controlled != "" {
        return controlled;
    }
    let selected = context["selected_path"];
    if type_of(selected) == "string" && selected != "" {
        return selected;
    }
    ()
}

fn direct_route_point(doc, path, route) {
    if type_of(path) != "string" || path == ""
        || type_of(route) != "string" || route == "" {
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
    let point = context_route_point_path(context, route);
    if point != () && direct_route_point(doc, point, route) { return point; }
    ()
}

fn context_route_point_path(context, route) {
    if context == () || type_of(route) != "string" || route == "" { return (); }
    let candidates = [];
    let target = context["target_path"];
    let hit = context["hit_path"];
    if type_of(target) == "string" && target != "" { candidates.push(target); }
    if type_of(hit) == "string" && hit != "" { candidates.push(hit); }
    let prim_paths = context["prim_paths"];
    if type_of(prim_paths) == "array" {
        for path in prim_paths {
            if type_of(path) == "string" && path != "" { candidates.push(path); }
        }
    }
    for path in candidates {
        let prefix = route + "/";
        if !path.starts_with(prefix) { continue; }
        let remainder = path.sub_string(prefix.len());
        if remainder != "" && remainder.index_of("/") < 0 { return path; }
    }
    ()
}

fn context_route_ribbon(context, route) {
    if context == () || type_of(route) != "string" || route == "" { return false; }
    let ribbon = ribbon_geometry_path(route);
    let candidates = [];
    let target = context["target_path"];
    let hit = context["hit_path"];
    if type_of(target) == "string" && target != "" { candidates.push(target); }
    if type_of(hit) == "string" && hit != "" { candidates.push(hit); }
    let prim_paths = context["prim_paths"];
    if type_of(prim_paths) == "array" {
        for path in prim_paths {
            if type_of(path) == "string" && path != "" { candidates.push(path); }
        }
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
    let has_pointer_target = false;
    for path in [context["target_path"], context["hit_path"]] {
        if type_of(path) == "string" && path != "" { has_pointer_target = true; }
    }
    let prim_paths = context["prim_paths"];
    if type_of(prim_paths) == "array" {
        for path in prim_paths {
            if type_of(path) == "string" && path != "" { has_pointer_target = true; }
        }
    }
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
        if used {
            index += 1;
            continue;
        }

        // Composed child enumeration omits inactive prims. Keep their names
        // reserved as well: reusing one would inherit its inactive opinion,
        // so the subsequent primOrder would name a non-child. Resolve only
        // inactive candidates; probing every already-active point through
        // USD made append cost grow with route length.
        let target = target_record(doc, path, "@runtime@");
        if !target.ok { return (); }
        if !target.exists { return candidate; }
        index += 1;
    }
    ()
}

// Read direct route children and their composed facts with one multi-prim
// snapshot. Trigger children identify route points; marker names and generated
// view children are never treated as topology.
fn route_child_snapshot(doc, route, attrs, validate_scope) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    if result == () {
        return #{ ok: false, retryable: true,
            error: "route stage is not synchronized" };
    }
    if result.ok == false {
        return #{ ok: false, retryable: true, error: result.error };
    }
    if result.children == () {
        return #{ ok: false, retryable: true,
            error: "route child list is unavailable" };
    }
    if result.children.len() == 0 {
        return #{ ok: true, children: result.children, records: [], route_scope: false };
    }
    let batch = query("QueryUsdPrims", #{
        doc_id: doc,
        paths: result.children,
        attrs: attrs,
        rels: if validate_scope { ["inputs:subject"] } else { () },
        schemas: validate_scope,
        children: true,
    });
    if batch == () {
        return #{ ok: false, retryable: true,
            error: "route child snapshot is not synchronized" };
    }
    if batch.ok == false {
        return #{ ok: false, retryable: true, error: batch.error };
    }
    if batch.records == () || batch.records.len() != result.children.len() {
        return #{ ok: false, retryable: true,
            error: "route child snapshot returned an incomplete result" };
    }
    let route_scope = false;
    if validate_scope {
        for record in batch.records {
            if record_is_route_program(record, ()) {
                route_scope = true;
                break;
            }
        }
    }
    #{ ok: true, children: result.children, records: batch.records,
        route_scope: route_scope }
}

fn route_point_record(record) {
    record != () && record.path != () && record.children != ()
        && record.children.contains(record.path + "/Trigger")
}

// Read route points from a settled composed-stage generation. This feeds the
// disposable ribbon and edit plans; authored route points remain durable USD.
fn route_positions(doc, route) {
    if generation(doc) == () {
        return #{ ok: false, retryable: true,
            error: "route document generation is unavailable" };
    }
    let snapshot = route_child_snapshot(doc, route, ["xformOp:translate"], true);
    if snapshot.ok != true { return snapshot; }
    if snapshot.route_scope != true {
        return #{ ok: false, retryable: false,
            error: "selected scope is not an authored route program scope" };
    }
    let positions = [];
    let paths = [];
    for record in snapshot.records {
        if record.active != true || !route_point_record(record) { continue; }
        if record.attrs == () {
            return #{ ok: false, retryable: true,
                error: "route point has no authored position" };
        }
        let position = record.attrs["xformOp:translate"];
        if position == () {
            return #{ ok: false, retryable: true,
                error: "route point has no authored position" };
        }
        positions.push(position);
        paths.push(record.path);
    }
    #{ ok: true, paths: paths, points: positions, children: snapshot.children,
        route_scope: true }
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

fn point_literal(point) {
    "(" + point[0].to_string() + ", "
        + point[1].to_string() + ", " + point[2].to_string() + ")"
}

fn point_vectors_literal(points) {
    let text = "[";
    for i in 0..points.len() {
        if i > 0 { text += ", "; }
        text += point_literal(points[i]);
    }
    text + "]"
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

fn ribbon_seed_ops(route, points) {
    let ribbon = ribbon_path(route);
    let geometry = ribbon_geometry_path(route);
    let first = points[0];
    let local_points = [];
    let normals = [];
    for point in points {
        local_points.push([
            point[0] - first[0], point[1] - first[1], point[2] - first[2],
        ]);
        normals.push([0.0, 1.0, 0.0]);
    }
    [
        #{ AddPrim: #{
            edit_target: "@view@",
            parent_path: route,
            name: "RouteRibbon",
            type_name: "Xform",
            reference: "lunco://markers/route_ribbon.usda",
            reference_prim_path: "",
        }},
        #{ SetTranslate: #{ edit_target: "@view@", path: ribbon, value: first }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "type", type_name: "token", value: "\"linear\"" }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "wrap", type_name: "token", value: "\"nonperiodic\"" }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "curveVertexCounts", type_name: "int[]",
            value: "[" + points.len().to_string() + "]" }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "points", type_name: "point3f[]",
            value: point_vectors_literal(local_points) }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "widths", type_name: "float[]",
            value: "[" + route_ribbon_width().to_string() + "]" }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "normals", type_name: "normal3f[]",
            value: point_vectors_literal(normals) }},
        #{ SetApiSchemas: #{ edit_target: "@view@", path: geometry,
            schemas: ["MaterialBindingAPI", "LunCoSurfaceAPI", "LunCoPointerInteractionAPI"] }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "lunco:interaction:left", type_name: "token", value: "\"pass_through\"" }},
        #{ SetAttribute: #{ edit_target: "@view@", path: geometry,
            name: "lunco:interaction:right", type_name: "token", value: "\"context\"" }},
    ]
}

// Sparse route points are prepared by the presentation owner from immutable
// terrain frame and coverage facts. The document contains the standard USD curve
// identity and interaction schema needed for picking and material binding.
fn update_ribbon_view(doc, route, points) {
    let ribbon = ribbon_path(route);
    let geometry = ribbon_geometry_path(route);
    let root_entity = find_path_in_document(doc, ribbon);
    let curve_entity = find_path_in_document(doc, geometry);
    if root_entity >= 0 && curve_entity >= 0 {
        let result = cmd("UpdateUsdCurveView", #{
            doc_id: doc,
            root_entity_id: root_entity,
            curve_entity_id: curve_entity,
            points: points,
            width_m: route_ribbon_width(),
        });
        if result == () || result.ok != true {
            return #{ ok: false, retryable: true,
                error: if result == () { "route mesh update returned no result" } else { result.error } };
        }
        return #{ ok: true, pending: false };
    }
    if !prim_exists(doc, ribbon) {
        if points.len() < 2 { return #{ ok: true, pending: false }; }
        let result = apply_view_ops(doc, "Create route ribbon view", ribbon_seed_ops(route, points));
        if result.ok != true { return result; }
        return #{ ok: true, pending: true };
    }
    #{ ok: true, pending: true }
}

fn marker_ops(doc, route, position, read) {
    if read == () || read.ok != true || read.route_scope != true {
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
    // route_positions already proved that this composed program scope and its
    // direct route children are present in the owning document snapshot.
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
    let entity_id = find_path_in_document(doc, point);
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
    if result.ok {
        result.path = marker.path;
    }
    result
}

fn on_insert_before(context) { insert_route_point(context, true) }
fn on_insert_after(context) { insert_route_point(context, false) }

fn preview_path(route) { route + "/MovePreview" }

// The view-only preview owns the waypoint being moved. UI selection may change
// while the preview is armed and is not the operation's target state.
fn move_preview_target(doc, route) {
    let preview = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: preview_path(route),
        edit_target: "@view@",
        authored_children: true,
    });
    if preview == () || preview.ok == false || preview.authored_children == () {
        return #{ ok: false, error: "move preview target state is unavailable" };
    }
    if preview.authored_here != true { return #{ ok: true, armed: false }; }
    let prefix = preview_path(route) + "/Target_";
    let point = ();
    for child in preview.authored_children {
        if !child.starts_with(prefix) { continue; }
        if point != () {
            return #{ ok: false, error: "move preview has more than one waypoint target" };
        }
        let name = child_name(child);
        let suffix = name.sub_string("Target_".len());
        point = route + "/" + suffix;
    }
    if point == () { return #{ ok: true, armed: false }; }
    if !direct_route_point(doc, point, route) {
        return #{ ok: false, error: "move preview target is not a direct route point" };
    }
    #{ ok: true, armed: true, point: point }
}

fn move_interaction_id(route) {
    "scene_interaction:route-point-move:" + route
}

fn clear_move_preview(doc, route) {
    let subscription = cmd("ClearScenePointerMoveHook", #{
        doc_id: doc,
        interaction_id: move_interaction_id(route),
    });
    if subscription == () || subscription.ok == false {
        return #{ ok: false,
            error: "scene pointer move subscription could not be cleared: "
                + (if subscription == () { "no command result" }
                    else if subscription.error != () { subscription.error }
                    else { "status " + subscription.status }) };
    }
    remove_move_preview(doc, route)
}

fn remove_move_preview(doc, route) {
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
    if context == () || context.doc_id == () {
        return failure("route pointer movement requires its owning document");
    }
    let route = context["route_path"];
    if type_of(route) != "string" || route == "" {
        return failure("route pointer movement requires its explicit route context");
    }
    let target = move_preview_target(context.doc_id, route);
    if target.ok != true { return target; }
    if target.armed != true {
        return #{ ok: true, changed: false };
    }
    let pointer = pointer_point(context);
    if pointer.ok != true { return #{ ok: true, changed: false }; }
    let preview = find_path_in_document(context.doc_id, preview_path(route));
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
    if result.ok {
        result.path = marker.path;
    }
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
    if result.ok {
        result.path = point.path;
    }
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
    // Selection belongs to the currently focused editor surface. The hit path
    // identifies the route target; it does not transfer selection ownership.
    let editor = query("InspectUsdSelection", #{});
    if editor == () || editor.ok == false {
        return failure("the editor selection owner is unavailable");
    }
    let result = if editor.focused == true && editor.doc_id == context.doc_id {
        cmd("SelectUsdPrim", #{
            preview: editor.preview,
            path: point,
            extend: false,
            toggle: false,
        })
    } else {
        // Resolve the canonical live entity instead of trusting the nested
        // visual or lower object chosen by scene picking.
        let entity_id = find_path_in_document(context.doc_id, point);
        if entity_id < 0 {
            return #{ ok: true, path: point, selected: false, pending: true };
        }
        cmd("SelectEntity", #{
            entity_id: entity_id,
            extend: false,
            toggle: false,
            remove_only: false,
            inspector_part_entity_id: 0,
        })
    };
    if result == () {
        return failure("the route selection command is unavailable in this host");
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
    if result.ok == true && !is_unattended() {
        let subscription = cmd("SetScenePointerMoveHook", #{
            doc_id: context.doc_id,
            interaction_id: move_interaction_id(route),
            tool: "scene_interaction",
            hook: "on_pointer_move",
            context: #{ route_path: route },
        });
        if subscription == () || subscription.ok != true {
            remove_move_preview(context.doc_id, route);
            let detail = if subscription == () { "no command result" }
                else if subscription.error != () { subscription.error }
                else { "status " + subscription.status };
            return failure("scene pointer move subscription could not be registered: "
                + detail);
        }
    }
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
