// Generic scene-level route program.
//
// A Route is a twin-authored Scope with a Program child and ordered route-point
// children. The program references its subject through `inputs:subject`; the
// subject does not own the route or this program. Route points are ordinary USD
// prims with a generic trigger-zone sensor. Their enter event advances the
// route, while the subject's authored Modelica drive law consumes the current
// waypoint setpoint.

const ROUTE_PROGRAM = "/Program";

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
    if route == () { return []; }
    let result = query("QueryUsdPrim", #{ path: route, children: true });
    if result == () || result.ok == false || result.children == () { return []; }
    let points = [];
    for path in result.children {
        let parts = path.split("/");
        let leaf = parts[parts.len() - 1];
        if leaf != "Program" { points.push(path); }
    }
    points
}

fn route_subject(me) {
    let route = route_path(me);
    if route == () { return -1; }
    let result = query("QueryUsdPrim", #{
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
    if route == () { return false; }
    let result = query("QueryUsdPrim", #{
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
    let points = route_children(me);
    let index = if state.route_index == () { 0 } else { state.route_index };
    if index < 0 || index >= points.len() { return (); }
    points[index]
}

fn current_zone(me, state) {
    let point = current_point(me, state);
    if point == () { return (); }
    let result = query("QueryUsdPrim", #{
        path: point + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if result == () || result.ok == false || result.attrs == () {
        return ();
    }
    result.attrs["lunco:triggerZone"]
}

fn set_route_autopilot(subject, position) {
    modelica_waypoint(subject, position, 0.6, 2.5, 0.0)
}

fn route_entrant_matches(subject, entrant) {
    let current = entrant;
    for depth in 0..32 {
        if current == subject { return true; }
        let next = parent(current);
        if next == () || next < 0 { return false; }
        current = next;
    }
    false
}

fn drive_current_point(me, state) {
    let subject = route_subject(me);
    if subject < 0 { return; }

    // Possession is the manual-control authority gate. A human owner takes
    // precedence over the unattended Modelica waypoint controller; leaving
    // the existing autopilot target untouched lets it resume after release.
    if controller(subject) != () { return; }

    let point = current_point(me, state);
    if !state.enabled || point == () {
        modelica_waypoint_stop(subject);
        return;
    }
    let target = find_path(point);
    let position = if target >= 0 { world_pos(target) } else { () };
    if position == () {
        modelica_waypoint_stop(subject);
    } else {
        set_route_autopilot(subject, position);
    }
}

fn route_view_ready(me, state) {
    if state.ribbon_ready == true { return true; }
    let doc = route_document(me);
    if doc == () { return false; }
    let result = waypoint_editor::ensure_ribbon(#{
        doc_id: doc,
        route_path: route_path(me),
    });
    result != () && result.ok == true
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

fn direct_route_point(context, route) {
    if context == () || context.prim_paths == () { return (); }
    let prefix = route + "/";
    for path in context.prim_paths {
        if !path.starts_with(prefix) { continue; }
        let remainder = path.sub_string(prefix.len());
        if remainder.index_of("/") < 0
            && remainder != "Program"
            {
            return path;
        }
    }
    ()
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
        context.route_path = route;
        waypoint_editor::on_click(context);
        return;
    }

    // Secondary clicks are routed by the same event contract. The policy only
    // handles an authored direct child of this route, so a right click on a
    // rover or another scene object cannot accidentally select it here.
    if context.button == "secondary" && direct_route_point(context, route) != () {
        context.route_path = route;
        waypoint_editor::on_context(context);
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
    if subject < 0 || !route_entrant_matches(subject, evt.value) { return; }
    let point = current_point(me, state);
    if point == () { return; }
    let zone = current_zone(me, state);
    if zone == () || zone == "" { return; }
    let expected = "enter:" + zone;
    if evt.name == expected {
        return #{ kind: "advance", index: state.route_index + 1 };
    }
    ()
}

fn on_start(me) {
    this.route_index = 0;
    this.enabled = route_enabled_from_usd(me);
    this.ribbon_ready = route_view_ready(me, this);
}

fn on_event(me, evt) {
    route_pointer(me, evt);
    let transition = advance_on_sensor(me, evt, this);
    if transition == () { return; }
    if transition.kind == "toggle" {
        this.enabled = transition.enabled;
        emit(if this.enabled { "route_started" } else { "route_stopped" });
    } else if transition.kind == "start" {
        this.enabled = true;
    } else if transition.kind == "advance" {
        emit("route_point_reached", this.route_index);
        this.route_index = transition.index;
    }
}

// The action stays live so route edits are visible without rebuilding the
// subject. The point list and relationship are queried on each action pass;
// sensor events are the sole progression authority.
fn task(me) {
    forever(step(|m| {
        if !this.ribbon_ready { this.ribbon_ready = route_view_ready(m, this); }
        drive_current_point(m, this);
    }, |m| false))
}
