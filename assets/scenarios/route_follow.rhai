// Generic scene-level route program.
//
// A Route is a twin-authored Scope with a Program child and ordered route-point
// children. The program references its subject through `inputs:subject`; the
// subject does not own the route or this program. Route points are ordinary USD
// prims with a generic trigger-zone sensor. Their enter event advances the
// route, while the generic navigation command only computes the current
// control setpoint.

const ROUTE_PROGRAM = "/Program";

fn route_failure(message) {
    #{ ok: false, error: message }
}

fn route_view_failure(result, fallback) {
    let error = if result == () || result.error == () { fallback } else { result.error };
    let failure = route_failure(error);
    if result != () && result.retryable == true { failure.retryable = true; }
    failure
}

fn route_path(me) {
    let path = usd_path(me);
    if path == () { return (); }
    // A child LunCoProgramAPI is executed on its owning entity. For a route
    // that owner is the scene-level Route scope; accepting the program path as
    // well keeps the helper correct for direct program attachments.
    if path.ends_with(ROUTE_PROGRAM) {
        return path.sub_string(0, path.len() - ROUTE_PROGRAM.len());
    }
    path
}

fn route_children(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return []; }
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: route, children: true });
    if result == () || result.ok == false || result.children == () { return []; }
    let points = [];
    for path in result.children {
        let parts = path.split("/");
        let leaf = parts[parts.len() - 1];
        // The presentation asset is authored under the route scope so it can
        // share the route's document/frame, but it is not a route point. Keep
        // visual infrastructure out of the ordered control set.
        if leaf == "Program" || leaf == "RouteRibbon" { continue; }
        let point = query("QueryUsdPrim", #{ doc_id: doc, path: path });
        if point != () && point.ok != false && point.active == true {
            points.push(path);
        }
    }
    points
}

// Route topology is authored USD structure, not a per-tick navigation input.
// Keep one snapshot for the current document generation and refresh it only
// at that structural boundary.
fn route_children_for(me, state) {
    if state.route_points != () { return state.route_points; }
    let points = route_children(me);
    state.route_points = points;
    points
}

fn route_subject(me) {
    route_subject_state(me).subject
}

fn route_subject_for(me, state) {
    if state.subject_id != () && state.subject_id >= 0 {
        return state.subject_id;
    }
    let current = route_subject(me);
    if current >= 0 {
        state.subject_id = current;
        return current;
    }
    // A structural USD edit can briefly rebuild the composed relationship
    // before its replacement is projected. Keep the last canonical entity
    // identity for one-shot events and control cleanup; do not lose an input
    // merely because an unrelated route-point edit crossed that boundary.
    if state.subject_id != () && state.subject_id >= 0 {
        return state.subject_id;
    }
    -1
}

fn route_subject_state(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () {
        return #{ subject: -1, retryable: true,
            error: "the route document is not synchronized" };
    }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route + ROUTE_PROGRAM,
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
    #{ subject: subject, retryable: false, error: "" }
}

fn route_subject_state_for(me, state) {
    if state.subject_id != () && state.subject_id >= 0 {
        return #{ subject: state.subject_id, retryable: false, error: "" };
    }
    let route = state.route_path;
    let doc = state.route_document;
    if route == () || doc == () {
        return #{ subject: -1, retryable: true,
            error: "the route document is not synchronized" };
    }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route + ROUTE_PROGRAM,
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
    state.subject_id = subject;
    #{ subject: subject, retryable: false, error: "" }
}

// ReleaseControl is a discrete control transaction. A disabled route must not
// enqueue the same fence and actuator reset on every fixed tick.
fn ensure_route_safe_stop(me, state) {
    if state.safe_stopped == true { return #{ ok: true }; }
    let subject = route_subject_for(me, state);
    if subject < 0 {
        return #{ ok: false, retryable: true,
            error: "the route subject has no live projection" };
    }
    brake(subject);
    state.safe_stopped = true;
    #{ ok: true }
}

fn route_subject_path(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return (); }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route + ROUTE_PROGRAM,
        rels: ["inputs:subject"],
    });
    if result == () || result.ok == false || result.relationships == () { return (); }
    let targets = result.relationships["inputs:subject"];
    if targets == () || targets.len() != 1 { return (); }
    targets[0]
}

fn route_is_selected(me) {
    let route = route_path(me);
    let subject = route_subject_path(me);
    let doc = route_document(me);
    if route == () || subject == () || doc == () { return false; }
    // The generic scene selection is the live control-plane identity. It
    // survives source replacement without inventing a route-specific registry
    // and is also available to headless scene tests.
    let selection = query("InspectSelection", #{});
    if selection != () && selection.ok != false && selection.primary != () {
        let selected_path = usd_path(selection.primary);
        if selected_path == route + ROUTE_PROGRAM { return true; }
    }
    if selection != () && selection.ok != false && selection.paths != () {
        for selected_path in selection.paths {
            if selected_path == route + ROUTE_PROGRAM { return true; }
        }
    }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: subject,
        rels: ["programs"],
    });
    if result == () || result.ok == false || result.relationships == () { return false; }
    let programs = result.relationships["programs"];
    programs != () && programs.len() == 1 && programs[0] == route + ROUTE_PROGRAM
}

fn route_enabled_from_usd(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return false; }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route + ROUTE_PROGRAM,
        attrs: ["inputs:enabled"],
    });
    if result == () || result.ok == false || result.attrs == () {
        return true;
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

fn route_start_index(me, state) {
    let cursor = current_point_info(me, state);
    if cursor.path == () || state.sensor_entered == () { return cursor.index; }
    let zone = current_zone(me, state);
    if zone != () && state.sensor_entered[zone] == true {
        return cursor.index + 1;
    }
    cursor.index
}

fn drive_current_point(me, state) {
    if !state.enabled {
        return ensure_route_safe_stop(me, state);
    }
    let subject_state = route_subject_state_for(me, state);
    let subject = route_subject_for(me, state);
    if subject < 0 {
        let failure = route_failure(subject_state.error);
        failure.retryable = subject_state.retryable;
        return failure;
    }
    let point = current_point(me, state);
    if point == () {
        brake(subject);
        return;
    }
    let target = find_path(point);
    let position = if target >= 0 { world_pos(target) } else { () };
    if position == () {
        brake(subject);
        return route_failure("the current route point has no live projection");
    }
    let command = nav_command(subject, position, 0.6, 2.5);
    if command == () {
        brake(subject);
        return route_failure("the subject has no usable navigation surface");
    }
    drive(subject, command.throttle, command.steer);
    state.safe_stopped = false;
    #{ ok: true }
}

fn route_revision_for(state) {
    if state.route_document == () { return (); }
    usd_document_generation(state.route_document)
}

fn refresh_route_view(me, state) {
    let doc = state.route_document;
    let revision = route_revision_for(state);
    if doc == () { return route_failure("the route document is unavailable"); }
    if revision == () { return route_failure("the route document generation is unavailable"); }
    if state.route_revision == revision { return #{ ok: true, revision: revision }; }
    // A document generation may replace children, the subject relationship, or
    // authored enablement. Drop all derived facts before reading the snapshot.
    state.route_points = ();
    state.subject_id = ();
    state.safe_stopped = false;
    let result = waypoint_editor::ensure_ribbon(#{
        doc_id: doc,
        route_path: state.route_path,
    });
    if result == () || result.ok != true {
        return route_view_failure(result, "the route ribbon tool returned no result");
    }
    // The ribbon itself is a runtime edit and may advance the document
    // generation. Capture the post-edit revision so the task does not issue
    // the same write again on its next step.
    let post_revision = route_revision_for(state);
    state.route_points = route_children(me);
    state.subject_id = route_subject(me);
    #{ ok: true, revision: if post_revision == () { revision } else { post_revision } }
}

// The runtime program HUD emits generic semantic actions. This route policy
// owns the route-specific scope binding; the editor library remains generic.
fn runtime_program_action(me, action, state) {
    if action == () { return; }
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return; }
    if action == "program.toggle" {
        if !route_is_selected(me) { return; }
        apply_toggle_transition(me, state, !state.enabled);
        return;
    }
    if action == "program.add" {
        if !is_unattended() { cmd("FocusPanel", #{ id: "rhai_repl" }); }
        notify_kind("Add a program with program_editor::add(doc, host, name, source)", "info");
        return;
    }
    if action == "program.path.add" {
        if !is_unattended() { cmd("FocusPanel", #{ id: "rhai_repl" }); }
        notify_kind("Add a path point with Alt-click on the scene", "info");
        return;
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
        return;
    }
    let record = program_editor::program_record(doc, path);
    if !record.ok { return; }
    if action.starts_with(edit_prefix) {
        program_editor::edit(path);
    } else {
        program_editor::select(path);
    }
}

fn advance_on_sensor(me, evt, state) {
    // Selection is transient runtime state. Reconcile it at the event edge as
    // well as in the task so a program-selection event and an operator action
    // cannot cross in the same simulation update.
    if !state.selected {
        state.selected = route_is_selected(me);
        if !state.selected { return; }
    }
    // Discrete control is a semantic input edge. The controller publishes it
    // for every authored control surface, even when no port named `action`
    // exists; this program owns the route-specific meaning of F.
    if evt.name == "intent.edge" && evt.source == route_subject_for(me, state)
        && evt.value != ()
        && evt.value["intent"] == "action"
        && (evt.value["edge"] == "pressed" || evt.value["edge"] == "pulse") {
        return #{ kind: "toggle", enabled: !state.enabled };
    }
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
    let subject = route_subject_for(me, state);
    if subject < 0 {
        return route_failure("the route has no valid subject");
    }
    if is_controlled(subject) {
        return route_failure("the route subject is already controlled");
    }
    // Route-point presentation is a transient USD view and may still be
    // settling after an edit. Prepare it before taking control so a retry
    // cannot issue PossessVessel repeatedly after a partial start.
    let reset = waypoint_editor::reset_marker_states(
        route_document(me), route_path(me),
    );
    if reset.ok != true {
        return route_view_failure(reset, "the route-point state tool returned no result");
    }
    // An autonomous program is a control session, but it must not steal the
    // operator's camera. Possession still goes through the generic authority
    // command so the authored controller sees the same piloted gate as a human.
    let result = cmd("PossessVessel", #{ target: subject, bind_camera: false });
    if result == () || result.ok != true {
        return route_failure("the subject rejected autonomous control");
    }
    #{ ok: true, subject: subject }
}

fn apply_toggle_transition(me, state, enabled) {
    if enabled {
        let started = start_route(me, state);
        if started.ok == true {
            state.enabled = true;
            state.safe_stopped = false;
            state.route_index = route_start_index(me, state);
            state.current_path = ();
            state.sensor_entered = #{};
            state.route_error = "";
            state.start_requested = false;
            emit("route_started");
        } else if started.retryable == true {
            state.start_requested = true;
        } else if state.route_error != started.error {
            notify_kind("Route program stopped: " + started.error, "error");
            state.route_error = started.error;
        }
        return;
    }
    state.enabled = false;
    ensure_route_safe_stop(me, state);
    let subject = route_subject_for(me, state);
    if subject >= 0 { release_control_claim(subject); }
    emit("route_stopped");
}

fn on_start(me) {
    this.route_path = route_path(me);
    this.route_document = route_document(me);
    this.route_points = ();
    this.route_index = 0;
    this.current_path = ();
    this.subject_id = ();
    this.safe_stopped = false;
    this.sensor_entered = #{};
    this.enabled = route_enabled_from_usd(me);
    this.selected = route_is_selected(me);
    this.route_revision = ();
    this.route_error = "";
    this.start_requested = false;
    let view = refresh_route_view(me, this);
    if view.ok != true {
        if view.retryable != true {
            notify_kind("Route program stopped: " + view.error, "error");
            this.route_error = view.error;
        }
    } else {
        this.route_revision = view.revision;
    }
    ensure_route_safe_stop(me, this);
    // Hot-swapped programs expose their lifecycle boundary on the same typed
    // event bus consumed by the Twin. UI/tests can wait for this authored
    // readiness edge before sending a one-shot command or pointer gesture.
    emit("program.ready", #{
        path: route_path(me),
        generation: this.route_revision,
    });
}

fn on_event(me, evt) {
    if evt.name == "program.selected" {
        this.selected = evt.value != () && evt.value["path"] == route_path(me) + ROUTE_PROGRAM;
        if !this.selected && this.enabled {
            let subject = route_subject_for(me, this);
            if subject >= 0 { brake(subject); }
            if subject >= 0 { release_control_claim(subject); }
            this.enabled = false;
            emit("route_stopped");
        }
        return;
    }
    if evt.name == "runtime.ui.action" {
        if handle_camera_ui_action(evt.value) { return; }
        runtime_program_action(me, evt.value, this);
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
        apply_toggle_transition(me, this, transition.enabled);
    } else if transition.kind == "start" {
        let started = start_route(me, this);
        if started.ok == true {
            this.enabled = true;
            this.safe_stopped = false;
            this.route_index = route_start_index(me, this);
            this.current_path = ();
            this.sensor_entered = #{};
            this.route_error = "";
            this.start_requested = false;
            emit("route_started");
        } else if started.retryable == true {
            this.start_requested = true;
        } else if this.route_error != started.error {
            notify_kind("Route program stopped: " + started.error, "error");
            this.route_error = started.error;
        }
    } else if transition.kind == "advance" {
        let point = current_point(me, this);
        let visual = waypoint_editor::set_marker_state(
            route_document(me), point, true,
        );
        if visual.ok != true {
            if this.route_error != visual.error {
                notify_kind("Route program stopped: " + visual.error, "error");
                this.route_error = visual.error;
            }
            return;
        }
        emit("route_point_reached", this.route_index);
        this.route_index = transition.index;
        this.current_path = ();
    }
}

// The action stays live so route edits are visible without rebuilding the
// subject. The route revision is the invalidation boundary for the disposable
// ribbon; sensor events remain the sole progression authority.
fn task(me) {
    forever(step(|m| {
        // A start request is independent of the disposable ribbon view. Do
        // not let a concurrent ribbon rebuild consume the one-shot action;
        // retry the typed start transaction until its authored markers are
        // visible in the canonical USD projection.
        if this.start_requested && !this.enabled {
            if !this.selected { this.selected = route_is_selected(m); }
            if !this.selected { return false; }
            let started = start_route(m, this);
            if started.ok != true { return false; }
            this.enabled = true;
            this.route_index = route_start_index(m, this);
            this.current_path = ();
            this.sensor_entered = #{};
            this.route_error = "";
            this.start_requested = false;
            emit("route_started");
        }
        // The ribbon is a presentation of the authored program, not the
        // control authority. Keep it synchronized for every live program so
        // selecting a different program cannot make the previous view stale.
        // Only navigation below is gated by the active-program selection.
        let view = refresh_route_view(m, this);
        if view.ok != true {
            if view.retryable == true { return false; }
            if this.route_error != view.error {
                notify_kind("Route program stopped: " + view.error, "error");
                this.route_error = view.error;
            }
            return false;
        }
        this.route_revision = view.revision;
        // Selection state is a transient USD projection and may appear one
        // frame after a program hot-reload. Reconcile it at the same cadence
        // as the route view before driving; no route-specific Rust state is
        // needed.
        if !this.selected {
            this.selected = route_is_selected(m);
            if !this.selected { return false; }
        }
        if !this.enabled {
            let safe_stop = ensure_route_safe_stop(m, this);
            if safe_stop.ok != true { return false; }
            return;
        }
        let cursor = current_point_info(m, this);
        if cursor.path != () {
            this.current_path = cursor.path;
            this.route_index = cursor.index;
        }
        let drive_result = drive_current_point(m, this);
        if drive_result != () && drive_result.ok == false {
            if drive_result.retryable == true { return false; }
            if this.route_error != drive_result.error {
                notify_kind("Route program stopped: " + drive_result.error, "error");
                this.route_error = drive_result.error;
            }
        } else {
            this.route_error = "";
        }
    }, |m| false))
}

fn on_stop(me) {
    ensure_route_safe_stop(me, this);
}
