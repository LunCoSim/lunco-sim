// Generic scene-level route program.
//
// A Route is a twin-authored Scope with one executable program and ordered
// route-point children. The program references its subject through
// `inputs:subject`; the
// subject does not own the route or this program. Route points are ordinary USD
// prims with a generic trigger-zone sensor. Their enter event advances the
// route. The authored Modelica program owns the continuous control law; this
// policy only publishes one target setpoint when the target changes.

fn route_failure(message) {
    #{ ok: false, error: message }
}

fn has_schema(record, schema) {
    if record == () || record.ok == false || record.api_schemas == () { return false; }
    for value in record.api_schemas {
        if value == schema { return true; }
    }
    false
}

fn program_path(me) {
    let path = usd_path(me);
    if path == () { return (); }
    let own = query("QueryUsdPrim", #{ path: path, schemas: true, children: true });
    if has_schema(own, "LunCoProgramAPI") { return path; }
    if own == () || own.ok == false || own.children == () { return (); }
    for child in own.children {
        let record = query("QueryUsdPrim", #{ path: child, schemas: true });
        if has_schema(record, "LunCoProgramAPI") { return child; }
    }
    ()
}

fn parent_path(path) {
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

fn route_path(me) {
    parent_path(program_path(me))
}

// A route point is an ordinary direct child with the standard trigger-zone
// attribute. This keeps presentation prims and program implementations out of
// the ordered control set without relying on a prim-name convention.
fn is_route_point(doc, path) {
    if path == () || path == "" { return false; }
    let trigger = query("QueryUsdPrim", #{
        path: path + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if trigger != () && trigger.ok != false && trigger.attrs != ()
        && trigger.attrs["lunco:triggerZone"] != () {
        return true;
    }
    // Authoring queries are allowed to lead the disposable scene projection.
    // Resolve the trigger against the document/canonical stage so a referenced
    // point or a just-created point remains part of the policy's topology at
    // that handoff boundary.
    let resolved = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path + "/Trigger",
        edit_target: "@root@",
    });
    resolved != () && resolved.ok != false
        && (resolved.composed_exists == true || resolved.authored_in_document == true)
}

fn route_children(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return (); }
    // This is a live control/presentation read. A transient ribbon or marker
    // opinion can advance the document journal before the document-scoped
    // projection catches up; the composed path query is the authoritative
    // runtime topology during that handoff.
    let result = query("QueryUsdPrim", #{ path: route, children: true });
    if result == () || result.ok == false || result.children == () { return (); }
    let points = [];
    for path in result.children {
        if !is_route_point(doc, path) { continue; }
        let point = query("QueryUsdPrim", #{ path: path });
        if point == () || point.ok == false { return (); }
        if point.active == true { points.push(path); }
    }
    points
}

// Route topology is authored USD structure, not a per-tick navigation input.
// The cache is owned by the task state and refreshed at the document revision
// boundary. The helper itself remains pure because Rhai maps are values.
fn route_children_for(me, state) {
    if state.route_points != () { return state.route_points; }
    route_children(me)
}

fn route_subject_state_for(me, state) {
    // Entity ids are projection identities, not authored identities. Rebind
    // them through the canonical relationship path after every projection
    // handoff instead of retaining an entity that may have been despawned.
    let route = state.route_path;
    let doc = state.route_document;
    if route == () || doc == () {
        return #{ subject: -1, retryable: true,
            error: "the route document is not synchronized" };
    }
    if state.subject_path != () {
        let cached = find_path(state.subject_path);
        if cached >= 0 {
            return #{ subject: cached, path: state.subject_path,
                retryable: false, error: "" };
        }
    }
    let result = query("QueryUsdPrim", #{
        path: state.program_path,
        rels: ["inputs:subject"],
    });
    if result == () || result.ok == false || result.relationships == () {
        return #{ subject: -1, retryable: true,
            error: "the authored inputs:subject relationship is not synchronized" };
    }
    let targets = result.relationships["inputs:subject"];
    if targets == () || targets.len() == 0 {
        return #{ subject: -1, retryable: false,
            error: "the authored inputs:subject relationship is unavailable" };
    }
    let subject = find_path(targets[0]);
    if subject < 0 {
        return #{ subject: -1, retryable: true,
            error: "the route subject has no live projection" };
    }
    #{ subject: subject, path: targets[0], retryable: false, error: "" }
}

fn route_subject_for(me, state) {
    route_subject_state_for(me, state).subject
}

// ReleaseControl is a discrete control transaction. A disabled route must not
// enqueue the same fence and actuator reset on every fixed tick.
fn ensure_route_safe_stop(me, state) {
    if state.safe_stopped == true { return #{ ok: true, safe_stopped: true }; }
    let subject = route_subject_for(me, state);
    if subject < 0 {
        return #{ ok: false, retryable: true,
            error: "the route subject has no live projection" };
    }
    let result = modelica_waypoint_stop(subject);
    if result == () || result.ok != true {
        return route_failure("the subject rejected the route safe-stop command");
    }
    #{ ok: true, safe_stopped: true }
}

fn route_subject_path(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return (); }
    let result = query("QueryUsdPrim", #{
        path: program_path(me),
        rels: ["inputs:subject"],
    });
    if result == () || result.ok == false || result.relationships == () { return (); }
    let targets = result.relationships["inputs:subject"];
    if targets == () || targets.len() != 1 { return (); }
    targets[0]
}

fn program_subject_path(doc, path) {
    if doc == () || path == () || path == "" { return (); }
    let result = query("QueryUsdPrim", #{
        path: path,
        rels: ["inputs:subject"],
    });
    if result == () || result.ok == false || result.relationships == () {
        return ();
    }
    let targets = result.relationships["inputs:subject"];
    if targets == () || targets.len() != 1 { return (); }
    targets[0]
}

fn discover_subject_programs(doc, scope, subject, depth) {
    let programs = [];
    if doc == () || scope == () || scope == "" || depth > 32 { return programs; }
    let result = query("QueryUsdPrim", #{
        path: scope,
        children: true,
    });
    if result == () || result.ok == false || result.children == () {
        return programs;
    }
    for child in result.children {
        let record = query("QueryUsdPrim", #{
            path: child,
            schemas: true,
        });
        if has_schema(record, "LunCoProgramAPI")
            && program_subject_path(doc, child) == subject {
            programs.push(child);
        }
        let nested = discover_subject_programs(doc, child, subject, depth + 1);
        for program in nested {
            if !programs.contains(program) { programs.push(program); }
        }
    }
    programs
}

// An intent edge is addressed to the controlled subject. During a USD
// projection handoff the relationship query can be temporarily unavailable,
// but the typed event still carries its canonical target gid. Queue the
// transition for the task instead of dropping the operator action at that
// boundary; the task retries the authority claim when the composed projection
// is current again.
fn intent_targets_route_subject(me, evt, state) {
    let subject = route_subject_for(me, state);
    if subject >= 0 && evt.source == subject
        && (state.selected || route_is_selected(me)) { return true; }
    evt.value != () && evt.value["target_gid"] != ()
        && evt.value["target_gid"] == evt.source
        && (state.selected || route_is_selected(me))
}

fn subject_selects_unique_program(subject, program) {
    let result = query("QueryUsdPrim", #{
        path: subject,
        rels: ["programs"],
    });
    if result == () || result.ok == false || result.relationships == () {
        return false;
    }
    let programs = result.relationships["programs"];
    programs != () && programs.len() == 1 && programs[0] == program
}

fn route_selection_matches(path, route, doc) {
    if path == () || path == "" || route == () || route == "" {
        return false;
    }
    if path == route || path == program_path_from_route(route) {
        return true;
    }
    let prefix = route + "/";
    if !path.starts_with(prefix) { return false; }
    let remainder = path.sub_string(prefix.len());
    if remainder == "" || remainder.index_of("/") >= 0 { return false; }
    is_route_point(doc, path)
}

fn program_path_from_route(route) {
    let result = query("QueryUsdPrim", #{ path: route, children: true });
    if result == () || result.ok == false || result.children == () { return (); }
    for child in result.children {
        let record = query("QueryUsdPrim", #{ path: child, schemas: true });
        if has_schema(record, "LunCoProgramAPI") { return child; }
    }
    ()
}

fn route_is_selected(me) {
    let program = program_path(me);
    let route = route_path(me);
    if program == () || route == () { return false; }
    // The attended control target is normally the program's subject, not the
    // invisible program scope. Selecting the rover must therefore arm its
    // unique route program just like selecting the program prim itself.
    let subject = route_subject_path(me);
    let doc = route_document(me);
    if subject == () || doc == () { return false; }
    // The generic scene selection is the live control-plane identity. It
    // survives source replacement without inventing a route-specific registry
    // and is also available to headless scene tests.
    let selection = query("InspectSelection", #{});
    if selection != () && selection.ok != false && selection.primary != () {
        let selected_path = usd_path(selection.primary);
        if selected_path == program { return true; }
        if selected_path == subject && subject_selects_unique_program(subject, program) {
            return true;
        }
        if route_selection_matches(selected_path, route, doc) {
            return true;
        }
    }
    if selection != () && selection.ok != false && selection.paths != () {
        for selected_path in selection.paths {
            if selected_path == program { return true; }
            if selected_path == subject && subject_selects_unique_program(subject, program) {
                return true;
            }
            if route_selection_matches(selected_path, route, doc) {
                return true;
            }
        }
    }
    // The subject-side inverse relationship is preferred when the Twin authors
    // it. During a source/variant projection handoff it can be temporarily
    // unavailable, so discover the same standard inputs:subject bindings from
    // the composed document. A subject with one executable program is the
    // unambiguous attended route; multiple programs still require program
    // selection.
    let result = query("QueryUsdPrim", #{
        path: subject,
        rels: ["programs"],
    });
    if result != () && result.ok != false && result.relationships != () {
        let programs = result.relationships["programs"];
        if programs != () && programs.len() == 1 && programs[0] == program {
            return true;
        }
        if programs != () && programs.len() > 1 { return false; }
    }
    let discovered = discover_subject_programs(doc, "/", subject, 0);
    discovered.len() == 1 && discovered[0] == program
}

fn route_enabled_from_usd(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return false; }
    let result = query("QueryUsdPrim", #{
        path: program_path(me),
        attrs: ["inputs:enabled"],
    });
    if result == () || result.ok == false || result.attrs == () {
        return false;
    }
    let value = result.attrs["inputs:enabled"];
    value == () || value > 0.5
}

fn route_document(me) {
    let route = route_path(me);
    if route == () { return (); }
    let result = query("QueryUsdPrim", #{ path: route });
    if result == () || result.ok == false || result.doc_id == () { return (); }
    result.doc_id
}

fn current_point(me, state) {
    let info = current_point_info(me, state);
    info.path
}

fn current_point_info(me, state) {
    let points = route_children_for(me, state);
    if points == () {
        let index = if state.route_index == () { 0 } else { state.route_index };
        return #{ path: (), index: index };
    }
    // Keep the cursor attached to the USD prim path, not to an ordinal. A
    // delete or insertion before the current point must not silently retarget
    // the rover; if the current point itself disappears, the same ordinal is
    // the expected next target.
    if state.current_path != () {
        for i in 0..points.len() {
            if points[i] == state.current_path {
                return #{ path: state.current_path, index: i };
            }
        }
    }
    let index = if state.route_index == () { 0 } else { state.route_index };
    if index < 0 { index = 0; }
    if index >= points.len() {
        return #{ path: (), index: points.len() };
    }
    #{ path: points[index], index: index }
}

fn current_zone(me, state) {
    let point = current_point(me, state);
    let doc = state.route_document;
    if point == () || doc == () { return (); }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: point + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if result == () || result.ok == false || result.attrs == () {
        return ();
    }
    result.attrs["lunco:triggerZone"]
}

fn route_entrant_matches(subject, entrant) {
    if entrant == () || entrant < 0 { return false; }
    let current = entrant;
    for depth in 0..32 {
        if current == subject { return true; }
        let next = parent(current);
        if next == () || next < 0 { return false; }
        current = next;
    }
    false
}

fn route_start_progress(me, state) {
    let points = route_children_for(me, state);
    let visited = if state.visited_points == () { #{} } else { state.visited_points };
    if points == () || points.len() == 0 {
        return #{ index: 0, visited_points: visited };
    }
    let subject = route_subject_for(me, state);
    let index = 0;
    while index < points.len() {
        let point = points[index];
        if visited[point] == true {
            index += 1;
            continue;
        }
        let zone = query("QueryUsdPrim", #{
            path: point + "/Trigger",
            attrs: ["lunco:triggerZone", "radius"],
        });
        let entered = false;
        if state.sensor_entered != () && zone != () && zone.ok != false
            && zone.attrs != () && zone.attrs["lunco:triggerZone"] != () {
            entered = state.sensor_entered[zone.attrs["lunco:triggerZone"]] == true;
        }
        // A route can be started after the sensor's enter pulse was emitted
        // (for example, the rover was already parked on W0 when the operator
        // pressed F). Treat the authored trigger volume as an initial
        // occupancy snapshot only; after this transaction, Avian sensor
        // enter events remain the sole arrival/progression authority.
        let occupied = false;
        if !entered && subject >= 0 && zone != () && zone.ok != false
            && zone.attrs != () && zone.attrs["radius"] != () {
            let subject_position = world_pos(subject);
            let point_position = world_pos(find_path(point));
            if subject_position != () && point_position != () {
                let dx = subject_position[0] - point_position[0];
                let dz = subject_position[2] - point_position[2];
                let radius = zone.attrs["radius"];
                occupied = dx * dx + dz * dz <= radius * radius;
            }
        }
        if !entered && !occupied { break; }
        visited[point] = true;
        index += 1;
    }
    #{ index: index, visited_points: visited }
}

fn drive_current_point(me, state) {
    if !state.enabled {
        let result = ensure_route_safe_stop(me, state);
        if result.ok == true { state.safe_stopped = true; }
        return #{ result: result, state: state };
    }
    let subject_state = route_subject_state_for(me, state);
    let subject = route_subject_for(me, state);
    if subject < 0 {
        let failure = route_failure(subject_state.error);
        failure.retryable = subject_state.retryable;
        return #{ result: failure, state: state };
    }
    let point = current_point(me, state);
    if point == () {
        let stopped = ensure_route_safe_stop(me, state);
        if stopped.ok == true && !state.completed {
            state.completed = true;
            state.enabled = false;
            emit("route_completed");
        }
        if stopped.ok == true { state.safe_stopped = true; }
        return #{ result: stopped, state: state };
    }
    if state.commanded_path == point && state.commanded_view_key == state.route_view_key {
        return #{ result: #{ ok: true }, state: state };
    }
    let target = find_path(point);
    let position = if target >= 0 { world_pos(target) } else { () };
    if position == () {
        return #{
            result: route_failure("the current route point has no live projection"),
            state: state,
        };
    }
    let command = modelica_waypoint(subject, position, 0.6, 2.5, 0.0);
    if command == () || command.ok != true {
        return #{
            result: route_failure("the subject rejected the route target"),
            state: state,
        };
    }
    state.commanded_path = point;
    state.commanded_revision = state.route_revision;
    state.commanded_view_key = state.route_view_key;
    state.safe_stopped = false;
    #{ result: #{ ok: true }, state: state }
}

fn route_revision_for(state) {
    if state.route_document == () { return (); }
    usd_document_generation(state.route_document)
}

fn refresh_route_view(me, state, snapshot) {
    let doc = state.route_document;
    let revision = route_revision_for(state);
    if doc == () {
        return #{ result: route_failure("the route document is unavailable"), state: state };
    }
    if revision == () {
        return #{
            result: route_failure("the route document generation is unavailable"),
            state: state,
        };
    }
    // The document generation also advances for disposable view overlays. Read
    // the authored route geometry first and invalidate only when its typed
    // identity/placement key changes (or marker state is explicitly dirty).
    let read = if snapshot == () {
        waypoint_editor::route_positions(doc, state.route_path)
    } else { snapshot };
    if read == () || read.ok != true {
        return #{
            result: if read == () {
                route_failure("the route stage is not synchronized")
            } else { read },
            state: state,
        };
    }
    let view_key = waypoint_editor::route_view_key(read);
    if view_key == () {
        return #{
            result: route_failure("the route view key cannot be derived"),
            state: state,
        };
    }
    if state.route_view_key == view_key && !state.marker_sync_pending {
        return #{ result: #{ ok: true, revision: revision, view_key: view_key }, state: state };
    }
    // A document generation may replace children, the subject relationship, or
    // authored enablement. Drop all derived facts before reading the snapshot.
    state.subject_id = ();
    state.subject_path = ();
    state.route_points = read.paths;
    state.safe_stopped = false;
    let result = waypoint_editor::ensure_ribbon(#{
        doc_id: doc,
        route_path: state.route_path,
    });
    // The ribbon is a disposable presentation view. It must never prevent a
    // valid authored program from starting or advancing when terrain sampling
    // or transient rendering is still settling.
    if result == () || result.ok != true {
        let presentation_error = if result == () || result.error == () {
            "route presentation is not ready"
        } else { result.error };
        if result.retryable != true {
            emit("route_presentation_unavailable", presentation_error);
        }
    }
    let marker = waypoint_editor::sync_marker_states(
        doc, state.route_path, state.visited_points,
    );
    if marker == () || marker.ok != true {
        state.marker_sync_pending = true;
        if marker == () || marker.retryable != true {
            emit("route_presentation_unavailable", "route-point state is not ready");
        }
    } else {
        state.marker_sync_pending = false;
    }
    // The ribbon itself is a runtime edit and may advance the document
    // generation. Capture the post-edit revision for observers, while the
    // view key remains the idempotence boundary for this presentation.
    let post_revision = route_revision_for(state);
    state.route_view_key = view_key;
    let subject_state = route_subject_state_for(me, state);
    state.subject_id = subject_state.subject;
    if subject_state.path != () { state.subject_path = subject_state.path; }
    #{
        result: #{
            ok: true,
            revision: if post_revision == () { revision } else { post_revision },
            view_key: view_key,
        },
        state: state,
    }
}

// The runtime program HUD emits generic semantic actions. This route policy
// owns the route-specific scope binding; the editor library remains generic.
fn runtime_program_action(me, payload, state) {
    if type_of(payload) != "map" || type_of(payload["action"]) != "string" {
        return state;
    }
    let action = payload["action"];
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return state; }
    if action == "program.toggle" {
        if !route_is_selected(me) { return state; }
        return apply_toggle_transition(me, state, !state.enabled);
    }
    if action == "program.add" {
        if !is_unattended() { cmd("FocusPanel", #{ id: "rhai_repl" }); }
        notify_kind("Add a program with program_editor::add(doc, host, name, source)", "info");
        return state;
    }
    if action == "program.path.add" {
        if !is_unattended() { cmd("FocusPanel", #{ id: "rhai_repl" }); }
        notify_kind("Add a path point with Alt-click on the scene", "info");
        return state;
    }
    let select_prefix = "program.select.path.";
    let edit_prefix = "program.edit.path.";
    let path = if action.starts_with(select_prefix) {
        action.sub_string(select_prefix.len())
    } else if action.starts_with(edit_prefix) {
        action.sub_string(edit_prefix.len())
    } else {
        ()
    };
    if path == () || path == "" {
        return state;
    }
    let record = program_editor::program_record(doc, path);
    if !record.ok { return state; }
    if action.starts_with(edit_prefix) {
        program_editor::edit(path);
    } else {
        program_editor::select(path);
    }
    state
}

fn advance_on_sensor(me, evt, state) {
    // Discrete control is a semantic input edge. The controller publishes it
    // for every authored control surface, even when no port named `action`
    // exists; this program owns the route-specific meaning of F.
    if evt.name == "intent.edge" && intent_targets_route_subject(me, evt, state)
        && evt.value != ()
        && evt.value["intent"] == "action"
        && (evt.value["edge"] == "pressed" || evt.value["edge"] == "pulse") {
        return #{ kind: "toggle", enabled: !state.enabled };
    }
    // Selection is transient runtime state. Reconcile it at the event edge as
    // well as in the task so a program-selection event and an operator action
    // cannot cross in the same simulation update.
    let selected = state.selected || route_is_selected(me);
    if !selected { return; }
    if evt.name == "route_start" {
        return #{ kind: "start" };
    }
    if !state.enabled { return (); }
    if !evt.name.starts_with("enter:") { return; }
    let subject = route_subject_for(me, state);
    // Avian sensors report the collider that crossed the zone. A nested body
    // is still an entrant of the authored route subject; follow the generic
    // ECS parent chain instead of requiring every collider to be duplicated
    // as a route-specific relationship.
    if subject < 0 || !route_entrant_matches(subject, evt.value) { return; }
    let cursor = current_point_info(me, state);
    let point = cursor.path;
    if point == () { return; }
    let zone = current_zone(me, state);
    if zone == () || zone == "" { return; }
    let expected = "enter:" + zone;
    if evt.name == expected {
        return #{ kind: "advance", index: cursor.index + 1 };
    }
    ()
}

fn start_route(me, state) {
    let subject_state = route_subject_state_for(me, state);
    let subject = subject_state.subject;
    if subject < 0 {
        let failure = route_failure(subject_state.error);
        failure.retryable = subject_state.retryable;
        return failure;
    }
    let owner = owner_of(subject);
    let local = local_session_id();
    if owner != () && (local == () || owner != local) {
        return route_failure("the route subject is controlled by another session");
    }
    // Program execution is not avatar possession. The route writes its
    // authored guidance setpoint through the generic control surface while
    // SessionRegistry remains free to represent only the user's possession.
    // This keeps the HUD and the next possession click tied to the avatar,
    // while ReleaseControl can still fence one input batch before this policy
    // publishes the active target again.
    #{ ok: true, subject: subject }
}

fn begin_route_run(me, state, started) {
    let progress = route_start_progress(me, state);
    state.selected = true;
    state.enabled = true;
    state.safe_stopped = false;
    state.route_index = progress.index;
    state.current_path = ();
    state.commanded_path = ();
    state.commanded_revision = ();
    state.commanded_view_key = ();
    state.completed = false;
    state.visited_points = progress.visited_points;
    state.marker_sync_pending = true;
    state.sensor_entered = #{};
    state.route_error = "";
    state.start_requested = false;
    emit("route_started");
    state
}

fn apply_toggle_transition(me, state, enabled) {
    if enabled {
        let started = start_route(me, state);
        if started.ok == true {
            return begin_route_run(me, state, started);
        } else if started.retryable == true {
            state.start_requested = true;
        } else if state.route_error != started.error {
            notify_kind("Route program stopped: " + started.error, "error");
            state.route_error = started.error;
        }
        return state;
    }
    state.enabled = false;
    let stopped = ensure_route_safe_stop(me, state);
    if stopped.ok == true { state.safe_stopped = true; }
    emit("route_stopped");
    state
}

fn on_start(me, ctx) {
    this.route_path = route_path(me);
    this.program_path = program_path(me);
    this.route_document = route_document(me);
    this.route_points = ();
    this.route_index = 0;
    this.current_path = ();
    this.subject_id = ();
    this.subject_path = ();
    this.safe_stopped = true;
    this.commanded_path = ();
    this.commanded_revision = ();
    this.commanded_view_key = ();
    this.completed = false;
    this.visited_points = #{};
    this.marker_sync_pending = true;
    this.sensor_entered = #{};
    this.enabled = route_enabled_from_usd(me);
    this.selected = route_is_selected(me);
    this.route_revision = ();
    this.route_view_key = ();
    this.route_error = "";
    this.start_requested = false;
    let refreshed = refresh_route_view(me, this, ());
    this = refreshed.state;
    let view = refreshed.result;
    if view.ok != true {
        if view.retryable != true {
            emit("route_presentation_unavailable", view.error);
        }
    } else {
        this.route_revision = view.revision;
        this.route_view_key = view.view_key;
    }
    if this.selected {
        let stopped = ensure_route_safe_stop(me, this);
        if stopped.ok == true { this.safe_stopped = true; }
    }
    // Hot-swapped programs expose their lifecycle boundary on the same typed
    // event bus consumed by the Twin. UI/tests can wait for this authored
    // readiness edge before sending a one-shot command or pointer gesture.
    emit("program.ready", #{
        path: program_path(me),
        generation: this.route_revision,
    });
}

fn on_event(me, evt, ctx) {
    if evt.name == "usd.document.projected" && evt.value != ()
        && evt.value["doc_id"] == this.route_document {
        // Projection is the authoritative invalidation boundary for route
        // presentation. Compare its composed point key here, after the stage
        // is current, rather than polling topology from the control task. The
        // key makes a view edit's follow-up event idempotent even when several
        // layer changes share a document generation.
        let generation = evt.value["generation"];
        let read = waypoint_editor::route_positions(
            this.route_document, this.route_path,
        );
        if read == () || read.ok != true {
            this.route_revision = ();
            return;
        }
        let view_key = waypoint_editor::route_view_key(read);
        if view_key == () {
            this.route_revision = ();
            return;
        }
        if view_key == this.route_view_key && !this.marker_sync_pending {
            this.route_revision = generation;
            return;
        }
        this.route_revision = ();
        let refreshed = refresh_route_view(me, this, read);
        this = refreshed.state;
        let view = refreshed.result;
        if view.ok == true {
            this.route_revision = view.revision;
            this.route_view_key = view.view_key;
        } else if view.retryable != true && this.route_error != view.error {
            emit("route_presentation_unavailable", view.error);
        }
        return;
    }
    if evt.name == "program.selected" {
        this.selected = evt.value != () && evt.value["path"] == this.program_path;
        if !this.selected && this.enabled {
            let subject = route_subject_for(me, this);
            if subject >= 0 {
                let stopped = ensure_route_safe_stop(me, this);
                if stopped.ok == true { this.safe_stopped = true; }
            }
            this.enabled = false;
            emit("route_stopped");
        }
        return;
    }
    if evt.name == "runtime.ui.action" {
        this = runtime_program_action(me, evt.value, this);
        return;
    }
    if evt.name == "cmd:ReleaseControl" && this.enabled {
        // Releasing the avatar is a manual-control lifecycle edge, not a
        // program stop. ReleaseControl applies its generic safe state once;
        // clear the route's command cache so the next program step republishes
        // the active waypoint without claiming the user's session.
        this.commanded_path = ();
        this.commanded_revision = ();
        this.commanded_view_key = ();
        return;
    }
    // Avian owns zone occupancy. Keep route-zone enter pulses in the program's
    // live state even while disabled so starting from the initial point targets
    // the next point. This is runtime state only; no occupancy is written to
    // USD.
    if evt.name.starts_with("enter:") {
        let subject = route_subject_for(me, this);
        let zone = evt.name.sub_string(6);
        if subject >= 0 && route_entrant_matches(subject, evt.value) {
            if this.sensor_entered == () { this.sensor_entered = #{}; }
            this.sensor_entered[zone] = true;
        }
    }
    let transition = advance_on_sensor(me, evt, this);
    if transition == () { return; }
    if transition.kind == "toggle" {
        this = apply_toggle_transition(me, this, transition.enabled);
    } else if transition.kind == "start" {
        let started = start_route(me, this);
        if started.ok == true {
            this = begin_route_run(me, this, started);
        } else if started.retryable == true {
            this.start_requested = true;
        } else if this.route_error != started.error {
            notify_kind("Route program stopped: " + started.error, "error");
            this.route_error = started.error;
        }
    } else if transition.kind == "advance" {
        let point = current_point(me, this);
        if point != () {
            this.visited_points[point] = true;
            this.marker_sync_pending = true;
        }
        emit("route_point_reached", this.route_index);
        this.route_index = transition.index;
        this.current_path = ();
        this.commanded_path = ();
        this.commanded_revision = ();
        this.commanded_view_key = ();
        let points = route_children_for(me, this);
        if points != () && this.route_index >= points.len() {
            this.safe_stopped = false;
            this.enabled = false;
            let subject = route_subject_for(me, this);
            let stopped = ensure_route_safe_stop(me, this);
            if stopped.ok == true { this.safe_stopped = true; }
            if stopped.ok == true && !this.completed {
                this.completed = true;
                emit("route_completed");
            }
        }
    }
}

// The action stays live so route edits are visible without rebuilding the
// subject. The route revision is the invalidation boundary for the disposable
// ribbon; sensor events remain the sole progression authority.
// The task driver resolves this named callback against the scenario AST and
// binds its persistent state map as `this`, exactly like an anonymous closure.
fn route_task_tick(m) {
    // A start request is independent of the disposable ribbon view. Do
    // not let a concurrent ribbon rebuild consume the one-shot action;
    // retry the typed start transaction until its authored markers are
    // visible in the canonical USD projection.
    if this.start_requested && !this.enabled {
        if !this.selected { this.selected = route_is_selected(m); }
        if !this.selected { return false; }
        let started = start_route(m, this);
        if started.ok != true { return false; }
        this = begin_route_run(m, this, started);
    }
    // The ribbon is a presentation of the authored program, not the
    // control authority. Keep it synchronized for every live program so
    // selecting a different program cannot make the previous view stale.
    // Only navigation below is gated by the active-program selection.
    // Document-projection events normally refresh route topology and the
    // disposable view. This branch retries only an explicitly pending or
    // retryable projection handoff; stable routes do no per-tick scene query.
    if this.route_revision == () || this.marker_sync_pending {
        let refreshed = refresh_route_view(m, this, ());
        this = refreshed.state;
        let view = refreshed.result;
        if view.ok != true {
            if view.retryable == true { return false; }
            if this.route_error != view.error {
                emit("route_presentation_unavailable", view.error);
            }
            return false;
        }
        this.route_revision = view.revision;
        this.route_view_key = view.view_key;
    }
    // Selection state is a transient USD projection and may appear one
    // frame after a program hot-reload. Reconcile it at the same cadence
    // as the route view before driving; no route-specific Rust state is
    // needed.
    if !this.selected {
        this.selected = route_is_selected(m);
        if !this.selected { return false; }
    }
    if !this.enabled {
        // An inactive program is a passive policy instance. Another
        // program may be selected for the same subject, so it must not
        // overwrite that program's command surface with a safe stop.
        if !this.selected { return; }
        let safe_stop = ensure_route_safe_stop(m, this);
        if safe_stop.ok == true { this.safe_stopped = true; }
        if safe_stop.ok != true { return false; }
        return;
    }
    let cursor = current_point_info(m, this);
    if cursor.path != () {
        this.current_path = cursor.path;
        this.route_index = cursor.index;
    }
    let driven = drive_current_point(m, this);
    this = driven.state;
    let drive_result = driven.result;
    if drive_result != () && drive_result.ok == false {
        if drive_result.retryable == true { return false; }
        if this.route_error != drive_result.error {
            notify_kind("Route program stopped: " + drive_result.error, "error");
            this.route_error = drive_result.error;
        }
    } else {
        this.route_error = "";
    }
}

fn task(me, ctx) {
    forever(step(Fn("route_task_tick"), Fn("route_task_never_done")))
}

fn route_task_never_done(me) {
    false
}

fn on_stop(me, ctx) {
    ensure_route_safe_stop(me, this);
}
