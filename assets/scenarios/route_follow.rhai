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
        if leaf == "Program" { continue; }
        let point = query("QueryUsdPrim", #{ doc_id: doc, path: path });
        if point != () && point.ok != false && point.active == true {
            points.push(path);
        }
    }
    points
}

fn route_subject(me) {
    let route = route_path(me);
    let doc = route_document(me);
    if route == () || doc == () { return -1; }
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route + ROUTE_PROGRAM,
        rels: ["inputs:subject"],
    });
    if result == () || result.ok == false || result.relationships == () {
        return -1;
    }
    let targets = result.relationships["inputs:subject"];
    if targets == () || targets.len() == 0 { return -1; }
    find_path(targets[0])
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

fn drive_current_point(me, state) {
    let subject = route_subject(me);
    if !state.enabled {
        if subject >= 0 { brake(subject); }
        return;
    }
    if subject < 0 {
        return route_failure("the authored inputs:subject relationship is unavailable");
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
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: route });
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
        return route_failure("the route ribbon could not be projected");
    }
    // The ribbon itself is a runtime edit and may advance the document
    // generation. Capture the post-edit revision so the task does not issue
    // the same write again on its next step.
    let post_revision = route_revision(me);
    #{ ok: true, revision: if post_revision == () { revision } else { post_revision } }
}

fn route_scene_root(me) {
    let route = route_path(me);
    if route == () { return (); }
    let parts = route.split("/");
    if parts.len() <= 2 { return "/"; }
    let root = "";
    for i in 1..parts.len() - 1 {
        root += "/" + parts[i];
    }
    root
}

fn route_pointer_context(context, route) {
    // Event payload maps are immutable snapshots at the event boundary. Build
    // the tool context explicitly so the route policy owns the route binding
    // without mutating a telemetry payload shared with other subscribers.
    #{
        button: context.button,
        screen_position: context.screen_position,
        hit_entity_id: context.hit_entity_id,
        target_entity_id: context.target_entity_id,
        hit_path: context.hit_path,
        target_path: context.target_path,
        selected_entity_id: context.selected_entity_id,
        selected_path: context.selected_path,
        controlled_entity_id: context.controlled_entity_id,
        controlled_path: context.controlled_path,
        prim_paths: context.prim_paths,
        modifiers: context.modifiers,
        doc_id: context.doc_id,
        scene_root_path: context.scene_root_path,
        route_path: route,
        world_position: context.world_position,
    }
}

fn route_pointer(me, evt) {
    if evt.name != "scene.pointer" || evt.value == () { return; }
    let context = evt.value;
    let route = route_path(me);
    if route == () || context.scene_root_path == ()
        || context.scene_root_path != route_scene_root(me) {
        return;
    }

    let modifiers = context.modifiers;
    if context.button == "primary"
        && modifiers != ()
        && modifiers["alt"] == true
        && modifiers["shift"] != true
        && modifiers["ctrl"] != true
        && context.world_position != () {
        waypoint_editor::on_click(route_pointer_context(context, route));
        return;
    }

    // Secondary clicks are routed by the same event contract. The policy only
    // handles an authored direct child of this route, so a right click on a
    // rover or another scene object cannot accidentally select it here.
    if context.button == "secondary" {
        waypoint_editor::on_context(route_pointer_context(context, route));
    }
}

fn advance_on_sensor(me, evt, state) {
    // Discrete control is a semantic input edge. The controller publishes it
    // for every authored control surface, even when no port named `action`
    // exists; this program owns the route-specific meaning of F.
    if evt.name == "intent.edge" && evt.source == route_subject(me)
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
    let subject = route_subject(me);
    if subject < 0 || evt.value != subject { return; }
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

fn start_route(me) {
    let subject = route_subject(me);
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
    this.enabled = route_enabled_from_usd(me);
    this.route_revision = ();
    this.route_error = "";
    let view = refresh_route_view(me, this);
    if view.ok != true {
        notify_kind("Route program stopped: " + view.error, "error");
        this.route_error = view.error;
    } else {
        this.route_revision = view.revision;
    }
}

fn on_event(me, evt) {
    route_pointer(me, evt);
    let transition = advance_on_sensor(me, evt, this);
    if transition == () { return; }
    if transition.kind == "toggle" {
        if transition.enabled {
            let started = start_route(me);
            if started.ok == true {
                this.enabled = true;
                this.route_index = 0;
                this.current_path = ();
                this.route_error = "";
                emit("route_started");
            } else if this.route_error != started.error {
                notify_kind("Route program stopped: " + started.error, "error");
                this.route_error = started.error;
            }
        } else {
            this.enabled = false;
            let subject = route_subject(me);
            if subject >= 0 { brake(subject); }
            emit("route_stopped");
        }
    } else if transition.kind == "start" {
        let started = start_route(me);
        if started.ok == true {
            this.enabled = true;
            this.route_index = 0;
            this.current_path = ();
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
        let view = refresh_route_view(m, this);
        if view.ok != true {
            if this.route_error != view.error {
                notify_kind("Route program stopped: " + view.error, "error");
                this.route_error = view.error;
            }
            return false;
        }
        this.route_revision = view.revision;
        let cursor = current_point_info(m, this);
        if cursor.path != () {
            this.current_path = cursor.path;
            this.route_index = cursor.index;
        }
        let drive_result = drive_current_point(m, this);
        if drive_result != () && drive_result.ok == false {
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
    let subject = route_subject(me);
    if subject >= 0 { brake(subject); }
}
