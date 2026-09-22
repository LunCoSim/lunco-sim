// Production acceptance for Avian-driven route starts and preserved progress.
// The Rover moves through each trigger; this observer never manufactures
// route-zone events.

const ROOT = "/RouteProgress";
const ROUTE = ROOT + "/Route";
const SUBJECT_PATH = ROOT + "/Subject";

fn event_is_within(entity, ancestor) {
    if entity == () || entity < 0 || ancestor < 0 { return false; }
    let current = entity;
    for depth in 0..32 {
        if current == ancestor { return true; }
        let next = parent(current);
        if next == () || next < 0 { return false; }
        current = next;
    }
    false
}

fn point_paths(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE, children: true });
    let points = [];
    if result == () || result.ok == false || result.children == () { return points; }
    for path in result.children {
        let zone = query("QueryUsdPrim", #{
            doc_id: doc,
            path: path + "/Trigger",
            attrs: ["lunco:triggerZone"],
        });
        if zone != () && zone.ok != false && zone.attrs != ()
            && zone.attrs["lunco:triggerZone"] != () {
            points.push(path);
        }
    }
    points
}

fn point_position(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: ["xformOp:translate"],
    });
    if result == () || result.ok == false || result.attrs == () { return (); }
    result.attrs["xformOp:translate"]
}

fn marker_color(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path + "/Dome",
        attrs: ["primvars:displayColor"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["primvars:displayColor"] == ()
        || result.attrs["primvars:displayColor"].len() != 1 {
        return ();
    }
    result.attrs["primvars:displayColor"][0]
}

fn color_is(actual, expected) {
    actual != () && actual.len() == 3
        && (actual[0] - expected[0]).abs() < 0.000001
        && (actual[1] - expected[1]).abs() < 0.000001
        && (actual[2] - expected[2]).abs() < 0.000001
}

fn target_is(subject, doc, path) {
    let target_z = port(subject, "autopilot_target_z");
    let position = point_position(doc, path);
    target_z != () && position != ()
        && (target_z - position[2]).abs() < 0.001
}

fn finish(me, passed) {
    me.done = true;
    me.checks.push(t_true(passed, "route progress reaches its final verification"));
    report_verdict(me.checks, "ROUTE PROGRESS", "ROUTE_PROGRESS");
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.transition_elapsed = 0.0;
    this.phase = 0;
    this.done = false;
    this.checks = [];
    this.doc = -1;
    this.subject = find(SUBJECT_PATH);
    this.route_entity = find_path(ROUTE);
    this.start_entered = 0;
    this.point_one_entered = 0;
    this.started = 0;
    this.stopped = 0;
    this.reached = [];
}

fn on_event(me, evt, ctx) {
    if this.done { return; }
    if evt.name == "enter:route_P0"
        && event_is_within(evt.value, find(SUBJECT_PATH)) {
        this.start_entered += 1;
    }
    if evt.name == "enter:route_P1"
        && event_is_within(evt.value, find(SUBJECT_PATH)) {
        this.point_one_entered += 1;
    }
    if evt.source == this.route_entity {
        if evt.name == "route_started" { this.started += 1; }
        if evt.name == "route_stopped" { this.stopped += 1; }
        if evt.name == "route_point_reached" { this.reached.push(evt.value); }
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 45.0 {
        this.checks.push(t_true(false,
            "route progress acceptance timed out in phase " + this.phase.to_string()));
        finish(this, false);
        return;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        if route == () || route.ok == false || route.doc_id == () { return; }
        this.doc = route.doc_id;
        this.subject = find(SUBJECT_PATH);
        if this.subject < 0 { return; }
        let listed = program_editor::list(this.doc, ROUTE);
        if listed == () || !listed.ok || listed.programs.len() != 1 { return; }
        let selected = program_editor::select(listed.programs[0]["path"]);
        if selected == () || selected.ok != true {
            this.checks.push(t_true(false, "the route program can be selected"));
            finish(this, false);
            return;
        }
        this.checks.push(t_true(true, "the authored route program is selected"));
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        if this.start_entered == 0 { return; }
        this.checks.push(t_true(true,
            "Avian enter reaches the Rhai listener while the route is disabled"));
        let toggled = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
        if !toggled { finish(this, false); return; }
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        let points = point_paths(this.doc);
        let active = port(this.subject, "program_active");
        if this.started == 0 || points.len() != 3 || active == () || active < 0.5 {
            return;
        }
        let start_visited = color_is(marker_color(this.doc, points[0]), [0.55, 0.55, 0.55]);
        let starts_at_next = target_is(this.subject, this.doc, points[1]);
        if !start_visited || !starts_at_next { return; }
        this.checks.push(t_true(start_visited,
            "the initially occupied route point is marked visited"));
        this.checks.push(t_true(starts_at_next,
            "autopilot begins at the first route point after the occupied start"));
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        if this.point_one_entered == 0 || this.reached.len() != 1
            || this.reached[0] != 1 { return; }
        let points = point_paths(this.doc);
        if points.len() != 3 { return; }
        this.checks.push(t_true(
            color_is(marker_color(this.doc, points[1]), [0.55, 0.55, 0.55]),
            "the next sensor event marks its route point visited"));
        let stopped = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
        if !stopped { finish(this, false); return; }
        this.transition_elapsed = 0.0;
        this.phase = 4;
        return;
    }

    if this.phase == 4 {
        let active = port(this.subject, "program_active");
        if this.stopped == 0 || active == () || active > 0.5 { return; }
        let started = emit("runtime.ui.action", #{ action: "program.toggle", parameters: #{} });
        if !started { finish(this, false); return; }
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        this.transition_elapsed += dt();
        let active = port(this.subject, "program_active");
        if this.started < 2 || active == () || active < 0.5
            || this.transition_elapsed < 0.1 {
            return;
        }
        let points = point_paths(this.doc);
        if points.len() != 3 { return; }
        let resumed_at_next = target_is(this.subject, this.doc, points[2]);
        this.checks.push(t_true(resumed_at_next,
            "re-enabling skips every previously visited route point"));
        if !resumed_at_next { finish(this, false); return; }
        this.transition_elapsed = 0.0;
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        this.transition_elapsed += dt();
        if this.transition_elapsed < 0.5 { return; }
        let points = point_paths(this.doc);
        let visited_zone_did_not_repeat = this.point_one_entered == 1
            && this.reached.len() == 1 && this.reached[0] == 1
            && points.len() == 3 && target_is(this.subject, this.doc, points[2]);
        this.checks.push(t_true(visited_zone_did_not_repeat,
            "restarting inside a visited sensor keeps the next unvisited target"));
        finish(this, visited_zone_did_not_repeat);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.checks.push(t_true(false, "route progress test stopped before its verdict"));
        finish(this, false);
    }
}
