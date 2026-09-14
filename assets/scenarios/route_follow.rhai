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

fn route_subject(me) {
    route_subject_state(me).subject
}

fn route_subject_for(me, state) {
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

fn runtime_selected_program(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return (); }
    let parts = route.split("/");
    if parts.len() < 2 { return (); }
    let state = query("QueryUsdPrim", #{
        doc_id: doc,
        path: "/" + parts[1] + "/__LunCoRuntime/ProgramSelection",
        attrs: ["selectedPath"],
    });
    if state == () || state.ok == false || state.attrs == () { return (); }
    state.attrs["selectedPath"]
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
    let selected = runtime_selected_program(me);
    if selected != () && selected != "" {
        return selected == route + ROUTE_PROGRAM;
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
    let points = route_children(me);
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
    let doc = route_document(me);
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
    let subject_state = route_subject_state(me);
    let subject = route_subject_for(me, state);
    if !state.enabled {
        if subject >= 0 { brake(subject); }
        return;
    }
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
    #{ ok: true }
}

fn route_revision(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return (); }
    // The document generation is the route invalidation clock. Read it from
    // the document registry instead of QueryUsdPrim: the canonical stage may
    // be briefly behind after a typed edit, and a generation read must not
    // depend on that asynchronous projection settling.
    let result = query("InspectUsdDocument", #{ doc_id: doc });
    if result == () || result.ok == false || result.generation == () { return (); }
    result.generation
}

fn refresh_route_view(me, state) {
    let doc = route_document(me);
    let revision = route_revision(me);
    if doc == () { return route_failure("the route document is unavailable"); }
    if revision == () { return route_failure("the route document generation is unavailable"); }
    if state.route_revision == revision { return #{ ok: true, revision: revision }; }
    let result = waypoint_editor::ensure_ribbon(#{
        doc_id: doc,
        route_path: route_path(me),
    });
    if result == () || result.ok != true {
        return route_view_failure(result, "the route ribbon tool returned no result");
    }
    // The ribbon itself is a runtime edit and may advance the document
    // generation. Capture the post-edit revision so the task does not issue
    // the same write again on its next step.
    let post_revision = route_revision(me);
    #{ ok: true, revision: if post_revision == () { revision } else { post_revision } }
}

// The runtime program HUD emits generic semantic actions. This route policy
// owns the route-specific scope binding; the editor library remains generic.
fn runtime_program_action(me, action) {
    if action == () { return; }
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return; }
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
    // An autonomous program is a control session, but it must not steal the
    // operator's camera. Possession still goes through the generic authority
    // command so the authored controller sees the same piloted gate as a human.
    let result = cmd("PossessVessel", #{ target: subject, bind_camera: false });
    if result == () || result.ok != true {
        return route_failure("the subject rejected autonomous control");
    }
    #{ ok: true, subject: subject }
}

fn on_start(me) {
    this.route_index = 0;
    this.current_path = ();
    this.subject_id = route_subject(me);
    this.sensor_entered = #{};
    this.enabled = route_enabled_from_usd(me);
    this.selected = route_is_selected(me);
    this.route_revision = ();
    this.route_error = "";
    let view = refresh_route_view(me, this);
    if view.ok != true {
        if view.retryable != true {
            notify_kind("Route program stopped: " + view.error, "error");
            this.route_error = view.error;
        }
    } else {
        this.route_revision = view.revision;
    }
    // Hot-swapped programs expose their lifecycle boundary on the same typed
    // event bus consumed by the Twin. UI/tests can wait for this authored
    // readiness edge before sending a one-shot command or pointer gesture.
    emit("program.ready", #{
        path: route_path(me),
        generation: route_revision(me),
    });
}

fn on_event(me, evt) {
    if evt.name == "program.selected" {
        this.selected = evt.value != () && evt.value["path"] == route_path(me) + ROUTE_PROGRAM;
        if !this.selected && this.enabled {
            let subject = route_subject_for(me, this);
            if subject >= 0 { brake(subject); }
            this.enabled = false;
            emit("route_stopped");
        }
        return;
    }
    if evt.name == "runtime.ui.action" {
        runtime_program_action(me, evt.value);
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
        if transition.enabled {
            let started = start_route(me, this);
            if started.ok == true {
                this.enabled = true;
                this.route_index = route_start_index(me, this);
                this.current_path = ();
                this.sensor_entered = #{};
                this.route_error = "";
                emit("route_started");
            } else if this.route_error != started.error {
                notify_kind("Route program stopped: " + started.error, "error");
                this.route_error = started.error;
            }
        } else {
            this.enabled = false;
            let subject = route_subject_for(me, this);
            if subject >= 0 { brake(subject); }
            emit("route_stopped");
        }
    } else if transition.kind == "start" {
        let started = start_route(me, this);
        if started.ok == true {
            this.enabled = true;
            this.route_index = route_start_index(me, this);
            this.current_path = ();
            this.sensor_entered = #{};
            this.route_error = "";
            emit("route_started");
        } else if this.route_error != started.error {
            notify_kind("Route program stopped: " + started.error, "error");
            this.route_error = started.error;
        }
    } else if transition.kind == "advance" {
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
    let subject = route_subject_for(me, this);
    if subject >= 0 { brake(subject); }
}
