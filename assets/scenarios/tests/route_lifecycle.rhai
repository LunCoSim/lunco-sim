// Production acceptance for the generic route tool and scene-level route
// program. All observable route policy stays here; Rust is exercised only as
// the generic typed USD command and event/port boundary.

const ROOT = "/RouteLifecycle";
const ROUTE = "/RouteLifecycle/Route";
const SUBJECT_PATH = "/RouteLifecycle/Subject";
const RIBBON_GEOMETRY = "/RouteLifecycle/Route/RouteRibbon/Geometry";

fn direct_points(doc) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: ROUTE, children: true });
    let points = [];
    if result == () || result.ok == false || result.children == () { return points; }
    let prefix = ROUTE + "/";
    for path in result.children {
        let rest = path.sub_string(prefix.len());
        if rest != "" && rest.index_of("/") < 0
            && rest != "Program" && rest != "RouteRibbon"
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

fn ribbon_width(doc) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["widths"],
    });
    if result == () || result.ok == false || result.attrs == ()
        || result.attrs["widths"] == () { return (); }
    result.attrs["widths"]
}

fn ribbon_width_matches(doc, expected) {
    let widths = ribbon_width(doc);
    widths != () && widths.len() == 1 && (widths[0] - expected).abs() < 0.001
}

fn ribbon_normals_match(doc, count) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: RIBBON_GEOMETRY,
        attrs: ["normals"],
    });
    result != () && result.ok != false && result.attrs != ()
        && result.attrs["normals"] != () && result.attrs["normals"].len() == count
}

fn ribbon_anchor_matches_point(doc, point_path) {
    let point = query("QueryUsdPrim", #{
        doc_id: doc,
        path: point_path,
    });
    let ribbon = query("QueryUsdPrim", #{
        doc_id: doc,
        path: "/RouteLifecycle/Route/RouteRibbon",
    });
    if point == () || ribbon == () || point.world_position == ()
        || ribbon.world_position == () { return false; }
    let a = point.world_position;
    let b = ribbon.world_position;
    (a[0] - b[0]).abs() < 0.001
        && (a[1] - b[1]).abs() < 0.001
        && (a[2] - b[2]).abs() < 0.001
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
    this.active_edit = false;
    this.hold_started = 0;
    this.program_switch_complete = false;
    this.route_ready = false;
    this.program_switch_subject = -1;
}

fn on_event(me, evt) {
    if evt.name == "route_started" { this.started += 1; }
    if evt.name == "route_stopped" { this.stopped += 1; }
    if evt.name == "cmd:PossessVessel" { this.possessed += 1; }
    if evt.name == "route_point_reached" {
        this.reached += 1;
        this.reached_index = evt.value;
    }
    if evt.name == "program_switch_hold_started" { this.hold_started += 1; }
    if evt.name == "program.ready" && this.phase >= 101
        && evt.value != () && evt.value["path"] == ROUTE {
        let current = query("InspectUsdDocument", #{ doc_id: this.doc });
        if current != () && current.ok != false
            && evt.value["generation"] == current.generation {
            this.route_ready = true;
        }
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
        if this.subject < 0 || point_count(this.doc) != 3 || ribbon_count(this.doc) != 3
            || !ribbon_normals_match(this.doc, 3)
            || !ribbon_anchor_matches_point(this.doc, ROUTE + "/P0") {
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
            "route program materializes an oriented ribbon from authored points"));
        this.checks.push(t_true(ribbon_width_matches(this.doc, 0.5),
            "route ribbon keeps a readable annotation width"));
        let listed = program_editor::list(this.doc, ROUTE);
        this.checks.push(t_true(listed.ok && listed.programs.len() == 1
            && listed.programs[0].path == ROUTE + "/Program",
            "generic program discovery lists the authored executable"));
        let hud = program_editor::enable_hud(this.doc, ROUTE, "always");
        this.checks.push(t_true(hud.ok, "Rhai can opt any USD program scope into the standard HUD"));
        if !hud.ok { finish(this); return; }
        this.phase = 50;
        return;
    }

    if this.phase == 50 {
        let hud_record = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE,
            attrs: ["lunco:ui:surfaceId", "lunco:ui:visibilityMode"],
        });
        if hud_record == () || hud_record.ok == false || hud_record.attrs == ()
            || hud_record.attrs["lunco:ui:surfaceId"] == ()
            || hud_record.attrs["lunco:ui:visibilityMode"] == () {
            return;
        }
        this.checks.push(t_true(hud_record != () && hud_record.ok != false
            && hud_record.attrs["lunco:ui:surfaceId"] == "program-browser"
            && hud_record.attrs["lunco:ui:visibilityMode"] == "always",
            "HUD activation is authored through the typed USD document"));
        for check in invalid_edit_checks(this.doc) { this.checks.push(check); }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        if !this.program_switch_complete {
            this.program_switch_subject = find(SUBJECT_PATH);
            this.route_ready = false;
            let switched = program_editor::switch_source(
                this.doc,
                ROUTE + "/Program",
                "lunco://scenarios/tests/program_switch_hold.rhai",
            );
            if !switched.ok { this.checks.push(t_true(false, switched.error)); finish(this); return; }
            this.phase = 100;
            return;
        }
        emit("scene.pointer", context(this.doc, (), [0.0, 1.0, -32.0]));
        this.phase = 2;
        return;
    }

    if this.phase == 100 {
        let program = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE + "/Program",
            attrs: ["info:implementationSource", "info:sourceAsset"],
        });
        if program == () || program.ok == false || program.attrs == ()
            || program.attrs["info:sourceAsset"] != "lunco://scenarios/tests/program_switch_hold.rhai"
            || this.hold_started == 0 {
            return;
        }
        this.checks.push(t_true(true,
            "runtime USD source switching activates a new authored Rhai program"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.program_switch_subject,
            "switching a program preserves the subject entity identity"));
        let restored = program_editor::switch_source(
            this.doc,
            ROUTE + "/Program",
            "lunco://scenarios/route_follow.rhai",
        );
        if !restored.ok { this.checks.push(t_true(false, restored.error)); finish(this); return; }
        this.phase = 101;
        return;
    }

    if this.phase == 101 {
        let program = query("QueryUsdPrim", #{
            doc_id: this.doc,
            path: ROUTE + "/Program",
            attrs: ["info:implementationSource", "info:sourceAsset"],
        });
        if program == () || program.ok == false || program.attrs == ()
            || program.attrs["info:sourceAsset"] != "lunco://scenarios/route_follow.rhai" {
            return;
        }
        this.checks.push(t_true(true,
            "the original program source can be restored without rebuilding its subject"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.program_switch_subject,
            "program replacement does not recreate the physical subject"));
        // Wait for the restored program's lifecycle start before delivering a
        // one-shot scene event. A semantic event is not a durable command and
        // must not be sent into a program that is still being replaced.
        if !this.route_ready { return; }
        emit("scene.pointer", context(this.doc, (), [0.0, 1.0, -32.0]));
        this.program_switch_complete = true;
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        if point_count(this.doc) != 4 || !has_path(this.doc, this.new_point)
            || ribbon_count(this.doc) != 4 || !ribbon_normals_match(this.doc, 4) { return; }
        this.checks.push(t_true(true,
            "Alt-click policy appends a named route point"));
        let position = point_position(this.doc, this.new_point);
        this.checks.push(t_true(position != () && position[2] == -32.0,
            "appended point has the clicked world position"));
        let secondary = context(this.doc, this.new_point, ());
        secondary.button = "secondary";
        secondary.modifiers = #{ alt: false, shift: false, ctrl: false };
        emit("scene.pointer", secondary);
        this.phase = 20;
        return;
    }

    if this.phase == 20 {
        if is_unattended() {
            this.checks.push(t_true(true,
                "secondary-click validates the route context in unattended mode"));
            this.phase = 21;
            return;
        }
        let selection = query("InspectSelection", #{});
        let expected = find_path(this.new_point);
        if selection == () || selection.ok == false || selection.primary != expected { return; }
        this.checks.push(t_true(true,
            "secondary-click routes through the generic scene pointer policy"));
        this.phase = 21;
        return;
    }

    if this.phase == 21 {
        let selected = waypoint_editor::context_route_point(
            context(this.doc, this.new_point, ()), ROUTE,
        );
        this.checks.push(t_true(selected == this.new_point,
            "secondary-click policy selects the direct route point"));
        if find_path(this.new_point) < 0 { return; }
        let nested = context(this.doc, this.new_point + "/Dome", ());
        nested.target_path = this.new_point + "/Dome";
        nested.hit_path = this.new_point + "/Dome";
        nested.prim_paths = [this.new_point + "/Dome", this.new_point];
        let selected_nested = waypoint_editor::context_route_point(nested, ROUTE);
        this.checks.push(t_true(selected_nested == this.new_point,
            "secondary-click resolves nested marker visuals to the canonical route point"));
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
        let moved = waypoint_editor::on_move(context(
            this.doc, ROUTE + "/P2", [2.0, 1.0, -26.0],
        ));
        this.checks.push(t_true(moved.ok == true && moved.path == ROUTE + "/P2",
            "moving a referenced route point authors a local USD transform opinion"));
        this.phase = 40;
        return;
    }

    if this.phase == 40 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 2.0 || position[2] != -26.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "moving a referenced route point updates its composed ribbon"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "moving a referenced point preserves the subject identity"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if undone == () || undone.ok != true { return; }
        this.phase = 41;
        return;
    }

    if this.phase == 41 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 0.0 || position[2] != -24.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "undo removes only the local transform opinion from a referenced point"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "undoing a referenced transform preserves the subject identity"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if redone == () || redone.ok != true { return; }
        this.phase = 42;
        return;
    }

    if this.phase == 42 {
        let position = point_position(this.doc, ROUTE + "/P2");
        if position == () || position[0] != 2.0 || position[2] != -26.0
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "redo reapplies the referenced transform without scene reprojection"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "redoing a referenced transform preserves the subject identity"));
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
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if undone == () || undone.ok != true { return; }
        this.phase = 51;
        return;
    }

    if this.phase == 51 {
        if point_count(this.doc) != 3 || !has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "undo restores a referenced route point and its ribbon"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "undoing a referenced route edit preserves the subject identity"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if redone == () || redone.ok != true { return; }
        this.phase = 52;
        return;
    }

    if this.phase == 52 {
        if point_count(this.doc) != 2 || has_path(this.doc, ROUTE + "/P2")
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "redo reapplies the referenced route-point deletion"));
        this.checks.push(t_true(find(SUBJECT_PATH) == this.subject,
            "redoing a referenced route edit preserves the subject identity"));
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
        this.phase = 90;
        return;
    }

    if this.phase == 90 {
        if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
        let edge = intent_pulse(this.subject, "action");
        this.checks.push(t_true(edge != () && edge.ok != false,
            "F is delivered through the generic semantic action edge"));
        this.phase = 10;
        return;
    }

    if this.phase == 10 {
        if !this.active_edit {
            if point_count(this.doc) != 2 || ribbon_count(this.doc) != 2 { return; }
            this.checks.push(t_true(true,
                "modifier-combined Alt-click does not append a route point"));
            let added = waypoint_editor::on_click(
                context(this.doc, (), [0.0, 1.0, -24.0]),
            );
            this.checks.push(t_true(added.ok == true,
                "editing a route while it is running appends a point"));
            this.active_edit = true;
            return;
        }
        if point_count(this.doc) != 3 || ribbon_count(this.doc) != 3 { return; }
        if this.started == 0 || this.stopped != 0 { return; }
        let throttle = port(this.subject, "throttle");
        if throttle == () || throttle.abs() < 0.01 { return; }
        this.checks.push(t_true(true,
            "an active route keeps running through a live point edit"));
        if this.possessed == 0 { return; }
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
        // The F edge was accepted in the previous phase; the route program
        // performs its safe-stop edge immediately and publishes
        // `route_stopped` on the shared event bus. Do not couple this edit
        // gate to a modelica port read after the route program has retired.
        this.checks.push(t_true(true, "F toggles the route program off"));
        this.checks.push(t_true(true,
            "route_stopped is emitted only after the generic brake policy runs"));
        let points = direct_points(this.doc);
        if points.len() == 0 { return; }
        let delete_path = points[0];
        let target = find_path(delete_path);
        if target < 0 { return; }
        this.generic_delete_path = delete_path;
        let deleted = cmd("DeleteEntity", #{ target: target });
        if deleted == () || deleted.ok == false {
            this.checks.push(t_true(false,
                "generic DeleteEntity accepted a selected authored route point"));
            finish(this);
            return;
        }
        this.phase = 13;
        return;
    }

    if this.phase == 13 {
        if point_count(this.doc) != 2 || has_path(this.doc, this.generic_delete_path)
            || ribbon_count(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "generic DeleteEntity persists a runtime active override for an authored point"));
        finish(this);
    }
}

fn on_stop(me) {
    if !this.done {
        this.checks.push(t_true(false, "route lifecycle test stopped before its verdict"));
        finish(this);
    }
}
