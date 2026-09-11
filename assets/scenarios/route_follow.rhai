// Generic scene-level route program.
//
// A Route is a twin-authored Scope with a Program child and ordered route-point
// children. The program references its subject through `inputs:subject`; the
// subject does not own the route or this program. Route points are ordinary USD
// prims with a generic trigger-zone sensor. Their enter event advances the
// route, while the generic navigation command only computes the current
// control setpoint.

const ROUTE_PROGRAM = "/Program";

fn route_path(me) { usd_path(me) }

fn route_children(me) {
    let route = route_path(me);
    if route == () { return []; }
    let result = query("QueryUsdPrim", #{ path: route, children: true });
    if result == () || result.ok != true || result.children == () { return []; }
    let points = [];
    for path in result.children {
        let leaf = path.sub_string(path.last_index_of("/") + 1);
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
    if result == () || result.ok != true || result.relationships == () {
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
    if result == () || result.ok != true || result.attrs == () {
        return true;
    }
    let value = result.attrs["inputs:enabled"];
    value == () || value > 0.5
}

fn current_point(me, state) {
    let points = route_children(me);
    if state.route_index == () { state.route_index = 0; }
    if state.route_index < 0 || state.route_index >= points.len() { return (); }
    points[state.route_index]
}

fn current_zone(me, state) {
    let point = current_point(me, state);
    if point == () { return (); }
    let result = query("QueryUsdPrim", #{
        path: point + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    if result == () || result.ok != true || result.attrs == () {
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
    let point = current_point(me, state);
    if subject < 0 || point == () {
        if subject >= 0 { brake(subject); }
        return;
    }
    let target = find_path(point);
    let position = if target >= 0 { world_pos(target) } else { () };
    if position == () {
        brake(subject);
        return;
    }
    let command = nav_command(subject, position, 0.6, 2.5);
    if command == () {
        brake(subject);
        return;
    }
    drive(subject, command.throttle, command.steer);
}

fn advance_on_sensor(me, evt, state) {
    if evt.name == "route_start" {
        state.enabled = true;
        return;
    }
    if !state.enabled { return; }
    if !evt.name.starts_with("enter:") { return; }
    let subject = route_subject(me);
    if subject < 0 || evt.value != subject { return; }
    let point = current_point(me, state);
    if point == () { return; }
    let zone = current_zone(me, state);
    if zone == () || zone == "" { return; }
    let expected = "enter:" + zone;
    if evt.name == expected {
        emit("route_point_reached", state.route_index);
        state.route_index += 1;
    }
}

fn on_start(me) {
    this.route_index = 0;
    this.enabled = route_enabled_from_usd(me);
}

fn on_event(me, evt) {
    advance_on_sensor(me, evt, this);
}

// The action stays live so route edits are visible without rebuilding the
// subject. The point list and relationship are queried on each action pass;
// sensor events are the sole progression authority.
fn task(me) {
    forever(step(|m| drive_current_point(m, this), |m| false))
}
