// Variant-composed route authoring acceptance. This mirrors the Summer Space
// School shape: a wrapper selects a terrain/site variant and the route points
// live below that composed arc. The test exercises add, move, delete, and
// undo/redo through the public Rhai/USD authoring surface.

const ROOT = "/RouteVariantLifecycle";
const ROUTE = ROOT + "/Route";
const SUBJECT = ROOT + "/Subject";
const P0 = ROUTE + "/P0";
const P1 = ROUTE + "/P1";
const P2 = ROUTE + "/P2";
const P3 = ROUTE + "/P3";

fn query_ok(value) { value != () && value.ok != false }

fn point(doc, path) {
    query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: ["xformOp:translate"],
    })
}

fn active_points(doc) {
    let route = query("QueryUsdPrim", #{
        doc_id: doc,
        path: ROUTE,
        children: true,
    });
    let count = 0;
    if route == () || route.ok == false || route.children == () { return count; }
    for path in route.children {
        let leaf = path.split("/")[path.split("/").len() - 1];
        if leaf == "Program" || leaf == "RouteRibbon" { continue; }
        let record = query("QueryUsdPrim", #{ doc_id: doc, path: path });
        if record != () && record.ok != false && record.active == true { count += 1; }
    }
    count
}

fn context(doc, path, position) {
    #{
        button: "primary",
        screen_position: [64.0, 64.0],
        doc_id: doc,
        scene_root_path: ROOT,
        route_path: ROUTE,
        controlled_path: SUBJECT,
        target_path: path,
        hit_path: path,
        prim_paths: if path == () { [] } else { [path] },
        world_position: point3(position, "active_physics"),
    }
}

fn finish(me, checks) {
    me.done = true;
    report_verdict(checks, "ROUTE VARIANT LIFECYCLE", "ROUTE_VARIANT_LIFECYCLE");
}

fn fail(me, message) { finish(me, [t_true(false, message)]); }

fn on_start(me) {
    this.done = false;
    this.phase = 0;
    this.elapsed = 0.0;
    this.doc = -1;
    this.original = [10.0, 1.0, -16.0];
    this.moved = [12.0, 1.0, -18.0];
    this.checks = [];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        fail(this, "variant route lifecycle timed out in phase " + this.phase);
        return;
    }

    if this.phase == 0 {
        let route = query("QueryUsdPrim", #{ path: ROUTE, children: true });
        if route == () || route.ok == false || route.doc_id == ()
            || route.children == () || active_points(route.doc_id) != 3 { return; }
        this.doc = route.doc_id;
        let target = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: P1,
            edit_target: "@root@",
        });
        this.checks.push(t_true(query_ok(target)
            && target.under_arc == true
            && target.edit_scope == "local_override",
            "variant route point resolves as a canonical local override"));
        let initial = point(this.doc, P1);
        if initial == () || initial.attrs == () { return; }
        this.checks.push(t_true(initial.attrs["xformOp:translate"] == this.original,
            "selected site variant exposes its authored point pose"));
        let ribbon = waypoint_editor::ensure_ribbon(#{
            doc_id: this.doc,
            route_path: ROUTE,
        });
        if !query_ok(ribbon) { fail(this, "variant route ribbon could not be created"); return; }
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        let moved = waypoint_editor::on_move(context(this.doc, P1, this.moved));
        if !query_ok(moved) { fail(this, "moving a variant-composed point was rejected"); return; }
        this.phase = 2;
        return;
    }

    if this.phase == 2 {
        let record = point(this.doc, P1);
        if record == () || record.attrs == ()
            || record.attrs["xformOp:translate"] != this.moved { return; }
        this.checks.push(t_true(true,
            "moving a variant-composed point authors a local transform opinion"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if !query_ok(undone) { fail(this, "UndoDocument rejected the variant point move"); return; }
        this.phase = 3;
        return;
    }

    if this.phase == 3 {
        let record = point(this.doc, P1);
        if record == () || record.attrs == ()
            || record.attrs["xformOp:translate"] != this.original { return; }
        this.checks.push(t_true(true,
            "undo removes only the variant point's local override"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if !query_ok(redone) { fail(this, "RedoDocument rejected the variant point move"); return; }
        this.phase = 4;
        return;
    }

    if this.phase == 4 {
        let record = point(this.doc, P1);
        if record == () || record.attrs == ()
            || record.attrs["xformOp:translate"] != this.moved { return; }
        this.checks.push(t_true(true,
            "redo reapplies the variant point override without reprojection"));
        let deleted = waypoint_editor::on_delete(context(this.doc, P1, ()));
        if !query_ok(deleted) { fail(this, "deleting a variant-composed point was rejected"); return; }
        this.phase = 5;
        return;
    }

    if this.phase == 5 {
        let record = query("QueryUsdPrim", #{ doc_id: this.doc, path: P1 });
        if record == () || record.ok == false || record.active != false
            || active_points(this.doc) != 2 { return; }
        this.checks.push(t_true(true,
            "deleting a variant point authors active=false in the local layer"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if !query_ok(undone) { fail(this, "UndoDocument rejected the variant point delete"); return; }
        this.phase = 6;
        return;
    }

    if this.phase == 6 {
        if active_points(this.doc) != 3 { return; }
        this.checks.push(t_true(true,
            "undo restores the variant point and its route membership"));
        let added = waypoint_editor::on_click(context(this.doc, (), [30.0, 1.0, -32.0]));
        if !query_ok(added) || added.path != P3 {
            fail(this, "adding a point to the variant-composed route was rejected");
            return;
        }
        this.phase = 7;
        return;
    }

    if this.phase == 7 {
        if active_points(this.doc) != 4 { return; }
        let moved = waypoint_editor::on_move(context(this.doc, P3, [32.0, 1.0, -34.0]));
        if !query_ok(moved) { fail(this, "moving the newly added variant route point failed"); return; }
        let deleted = waypoint_editor::on_delete(context(this.doc, P3, ()));
        if !query_ok(deleted) { fail(this, "deleting the newly added variant route point failed"); return; }
        this.phase = 8;
        return;
    }

    if this.phase == 8 {
        let record = query("QueryUsdPrim", #{ doc_id: this.doc, path: P3 });
        if record == () || record.ok == false || record.active != false
            || active_points(this.doc) != 3 { return; }
        let ribbon = waypoint_editor::ensure_ribbon(#{
            doc_id: this.doc,
            route_path: ROUTE,
        });
        if !query_ok(ribbon) { fail(this, "variant route ribbon refresh failed after edits"); return; }
        this.checks.push(t_true(true,
            "add, move, and delete keep the variant route and ribbon coherent"));
        finish(this, this.checks);
    }
}

fn on_stop(me) {
    if !this.done { fail(this, "variant route lifecycle stopped before its verdict"); }
}
