// Production acceptance for the generic route tool and scene-level route
// program. All observable route policy stays here; Rust is exercised only as
// the generic typed USD command and event/port boundary.

const ROOT = "/RouteLifecycle";
const ROUTE = "/RouteLifecycle/Route";
const SUBJECT_PATH = "/RouteLifecycle/Subject";
const RIBBON_GEOMETRY = "/RouteLifecycle/RouteRibbon_RouteLifecycle_Route/Geometry";

fn direct_points(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE, children: true });
    let points = [];
    if result == () || result.ok == false || result.children == () { return points; }
    let prefix = ROUTE + "/";
    for path in result.children {
        let rest = path.sub_string(prefix.len());
        if rest != "" && rest.index_of("/") < 0 && rest != "Program"
            && has_path(doc, path) {
            points.push(path);
        }
    }
    points
}

fn point_count(doc) { direct_points(doc).len() }

fn route_revision(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE });
    if result == () || result.ok == false { return (); }
    result.generation
}

fn ribbon_count(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["curveVertexCounts"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["curveVertexCounts"] == () { return (); }
    result.attrs["curveVertexCounts"][0]
}

fn has_path(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    result != () && result.ok != false && result.active == true
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

fn context(doc, path, position) {
    #{
        button: "primary",
        doc_id: doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        controlled_path: SUBJECT_PATH,
        target_path: path,
        hit_path: path,
        prim_paths: [path],
        target_entity_id: if path == () { () } else { find_path(path) },
        hit_entity_id: if path == () { () } else { find_path(path) },
        modifiers: #{ alt: true, shift: false, ctrl: false },
        world_position: position,
    }
}

fn modified_context(doc, path, position, modifiers) {
    let value = context(doc, path, position);
    value.modifiers = modifiers;
    value
}

fn invalid_edit_checks(doc) {
    let before = route_revision(doc);
    let missing_context = waypoint_editor::on_click(#{
        doc_id: doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        controlled_path: SUBJECT_PATH,
    });
    let missing_position = waypoint_editor::on_move(context(doc, "/RouteLifecycle/NotAPoint", ()));
    let program_delete = waypoint_editor::on_delete(context(doc, ROUTE + "/Program", ()));
    let after = route_revision(doc);
    [
        t_true(missing_context.ok == false, "click without a world position is rejected"),
        t_true(missing_position.ok == false, "move without a world position is rejected"),
        t_true(program_delete.ok == false, "delete cannot target the route program"),
        t_true(before == after, "invalid route edits do not advance the USD revision"),
    ]
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.checks = [];
    this.new_point = ROUTE + "/P3";
    this.subject = -1;
    this.started = 0;
    this.stopped = 0;
    this.possessed = 0;
    this.reached = 0;
    this.reached_index = ();
}

fn on_event(me, evt) {
    if evt.name == "route_started" { this.started += 1; }
    if evt.name == "route_stopped" { this.stopped += 1; }
    if evt.name == "cmd:PossessVessel" { this.possessed += 1; }
    if evt.name == "route_point_reached" {
        this.reached += 1;
        this.reached_index = evt.value;
    }
}

fn finish(me) {
    me.done = true;
    report_verdict(me.checks, "ROUTE LIFECYCLE", "ROUTE_LIFECYCLE");
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 45.0 {
        this.checks.push(t_true(false, "route lifecycle acceptance timed out in phase " + this.phase));
        finish(this);
        return;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        if route == () || route.ok == false || route.doc_id == () { return; }
        this.doc = route.doc_id;
        this.subject = find(SUBJECT_PATH);
        if this.subject < 0 || point_count(this.doc) != 3 || ribbon_count(this.doc) != 3 {
            return;
        }
        let program = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE + "/Program",
            attrs: ["info:implementationSource", "info:sourceAsset", "info:id", "info:sourceCode"],
        });
        this.checks.push(t_true(program != () && program.ok != false
            && program.attrs["info:implementationSource"] == "sourceAsset"
            && program.attrs["info:sourceAsset"] != ()
            && program.attrs["info:id"] == ()
            && program.attrs["info:sourceCode"] == (),
            "route program uses the sourceAsset-only USD contract"));
        this.checks.push(t_true(true,
            "route program materializes one ribbon vertex per authored point"));
        for check in invalid_edit_checks(this.doc) { this.checks.push(check); }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        emit("scene.pointer", context(this.doc, (), [0.0, 1.0, -32.0]));
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        if point_count(this.doc) != 4 || !has_path(this.doc, this.new_point)
            || ribbon_count(this.doc) != 4 { return; }
        this.checks.push(t_true(true,
            "Alt-click policy appends a named route point"));
        let position = point_position(this.doc, this.new_point);
        this.checks.push(t_true(position != () && position[2] == -32.0,
            "appended point has the clicked world position"));
        let selected = waypoint_editor::context_route_point(
            context(this.doc, this.new_point, ()), ROUTE,
        );
        this.checks.push(t_true(selected == this.new_point,
            "secondary-click policy resolves the direct route point"));
        let moved = waypoint_editor::on_move(context(this.doc, this.new_point, [4.0, 1.0, -32.0]));
        this.checks.push(t_true(moved.ok == true && moved.path == this.new_point,
            "route-point move is a typed USD edit"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "adding and moving a point preserve the live subject identity"));
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        let position = point_position(this.doc, this.new_point);
        if position == () || position[0] != 4.0 || ribbon_count(this.doc) != 4 { return; }
        this.checks.push(t_true(true, "moving a point refreshes the reusable ribbon in place"));
        let deleted = waypoint_editor::on_delete(context(this.doc, this.new_point, ()));
        this.checks.push(t_true(deleted.ok == true && deleted.path == this.new_point,
            "delete removes an authored route point"));
        this.phase = 4;
        return;
    }

    if this.phase == 4 {
        if point_count(this.doc) != 3 || has_path(this.doc, this.new_point)
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true, "deleting a point updates ribbon topology"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "deleting a point does not rebuild the subject"));
        let deleted = waypoint_editor::on_delete(context(this.doc, ROUTE + "/P2", ()));
        this.checks.push(t_true(deleted.ok == true,
            "referenced route point uses a local USD active override on delete"));
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true, "deactivating a referenced point does not rebuild the subject"));
        waypoint_editor::on_delete(context(this.doc, ROUTE + "/P1", ()));
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        if point_count(this.doc) != 1 || has_path(this.doc, ROUTE + "/P1")
            || ribbon_count(this.doc) != () { return; }
        this.checks.push(t_true(true, "a one-point route has no stale ribbon"));
        waypoint_editor::on_delete(context(this.doc, ROUTE + "/P0", ()));
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        if point_count(this.doc) != 0 || ribbon_count(this.doc) != () { return; }
        this.checks.push(t_true(true, "an empty route is safe and has no stale presentation"));
        waypoint_editor::on_click(context(this.doc, (), [0.0, 1.0, -8.0]));
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        if point_count(this.doc) != 1 || ribbon_count(this.doc) != () { return; }
        waypoint_editor::on_click(context(this.doc, (), [0.0, 1.0, -16.0]));
        this.phase = 9;
        return;
    }

    if this.phase == 9 {
        if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
        emit("scene.pointer", modified_context(this.doc, (), [0.0, 1.0, -24.0],
            #{ alt: true, shift: true, ctrl: true }));
        let edge = intent_pulse(this.subject, "action");
        this.checks.push(t_true(edge != () && edge.ok != false,
            "F is delivered through the generic semantic action edge"));
        this.phase = 10;
        return;
    }

    if this.phase == 10 {
        if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "modifier-combined Alt-click does not append a route point"));
        let throttle = port(this.subject, "throttle");
        if this.started == 0 || this.possessed == 0 || throttle == ()
            || throttle.abs() < 0.01 { return; }
        this.checks.push(t_true(true,
            "F starts the route through generic possession authority"));
        this.checks.push(t_true(true,
            "the running route program produces generic drive commands"));
        let points = direct_points(this.doc);
        if points.len() == 0 { return; }
        let zone = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: points[0] + "/Trigger",
            attrs: ["lunco:triggerZone"],
        });
        if zone == () || zone.ok == false || zone.attrs == ()
            || zone.attrs["lunco:triggerZone"] == () { return; }
        emit("enter:" + zone.attrs["lunco:triggerZone"], this.subject);
        this.phase = 11;
        return;
    }

    if this.phase == 11 {
        if this.reached == 0 { return; }
        this.checks.push(t_true(this.reached_index == 0,
            "the sensor event advances the stable current route point"));
        intent_pulse(this.subject, "action");
        this.phase = 12;
        return;
    }

    if this.phase == 12 {
        let throttle = port(this.subject, "throttle");
        if this.stopped == 0 || throttle == () || throttle.abs() >= 0.01 { return; }
        this.checks.push(t_true(true, "F toggles the route program off"));
        this.checks.push(t_true(true, "stopping the route applies a safe brake command"));
        finish(this);
    }
}

fn on_stop(me) {
    if !this.done {
        this.checks.push(t_true(false, "route lifecycle test stopped before its verdict"));
        finish(this);
    }
}
