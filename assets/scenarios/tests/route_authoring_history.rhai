// Production Rhai acceptance for route authoring history.
// The test creates an untitled USD document, uses the route tool to move and
// delete one directly authored point, and observes each undo/redo through the
// public document/query surface. Marker-reference creation is covered by the
// active route lifecycle test, where the canonical stage is mounted.

const ROOT = "/World/History";
const ROUTE = ROOT + "/Route";
const SUBJECT = ROOT + "/Subject";
const P0 = ROUTE + "/P0";

fn query_ok(value) {
    value != () && value.ok != false
}

fn find_usd_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" { return item.doc_id; }
    }
    -1
}

fn find_new_usd_document(initial) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" && !initial.contains(item.doc_id) {
            return item.doc_id;
        }
    }
    -1
}

fn prim(doc, path) {
    let result = assembly_edit::inspect(doc, path);
    if !query_ok(result) || result.prim == () { return (); }
    result.prim
}

fn has_prim(doc, path) {
    let result = prim(doc, path);
    result != () && result.exists == true && result.active == true
}

fn point_position(doc, path) {
    let record = prim(doc, path);
    if record == () || record.attributes == () { return (); }
    for attribute in record.attributes {
        if attribute.name.ends_with("xformOp:translate") { return attribute.value; }
    }
    ()
}

fn x_is(doc, path, expected) {
    let position = point_position(doc, path);
    if position == () { return false; }
    // NewDocument uses USD's default centimetre stage units. The typed route
    // tool receives active-frame metres and the document stores centimetres.
    let prefix = if expected == 0.0 { "(0.0," } else { "(300.0," };
    position.starts_with(prefix)
}

fn route_context(doc, path, position) {
    #{
        doc_id: doc,
        scene_root_path: "/World",
        route_path: ROUTE,
        controlled_path: SUBJECT,
        target_path: path,
        hit_path: path,
        prim_paths: [path],
        world_position: position,
    }
}

fn finish(me, checks) {
    me.done = true;
    report_verdict(checks, "ROUTE AUTHORING HISTORY", "ROUTE_AUTHORING_HISTORY");
}

fn fail(me, message) {
    finish(me, [t_true(false, message)]);
}

fn on_start(me) {
    this.done = false;
    this.phase = 0;
    this.elapsed = 0.0;
    this.initial_docs = [];
    this.scene_doc = -1;
    this.doc = -1;
    this.checks = [];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 25.0 {
        fail(this, "route authoring history timed out in phase " + this.phase);
        return;
    }

    if this.phase == 0 {
        let docs = query("ListOpenDocuments");
        if docs == () || docs.open_documents == () { return; }
        this.initial_docs = [];
        for item in docs.open_documents { this.initial_docs.push(item.doc_id); }
        this.scene_doc = find_usd_document();
        if this.scene_doc < 0 { return; }
        let created = cmd("NewDocument", #{ kind: "usd" });
        if !query_ok(created) {
            fail(this, "NewDocument did not create an untitled USD document");
            return;
        }
        this.phase = 1;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 1 {
        this.doc = find_new_usd_document(this.initial_docs);
        if this.doc < 0 { return; }
        let created = assembly_edit::add_prim(
            this.doc, "@root@", "/World", "History", "Scope", (),
            assembly_edit::describe(this.doc).generation,
        );
        if !query_ok(created) {
            fail(this, "route history document rejected its root scope");
            return;
        }
        this.phase = 2;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 2 {
        if !has_prim(this.doc, ROOT) { return; }
        let generation = assembly_edit::describe(this.doc).generation;
        let created = assembly_edit::batch(this.doc, "Create route history roots", [
            #{ AddPrim: #{
                edit_target: "@root@", parent_path: ROOT, name: "Route",
                type_name: "Scope", reference: (), reference_prim_path: "",
            }},
            #{ AddPrim: #{
                edit_target: "@root@", parent_path: ROOT, name: "Subject",
                type_name: "Xform", reference: (), reference_prim_path: "",
            }},
        ], generation);
        if !query_ok(created) {
            fail(this, "route history document rejected its route roots");
            return;
        }
        this.phase = 3;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 3 {
        if !has_prim(this.doc, SUBJECT) { return; }
        let added = assembly_edit::add_prim(
            this.doc, "@root@", ROUTE, "P0", "Xform", (),
            assembly_edit::describe(this.doc).generation,
        );
        if !query_ok(added) {
            fail(this, "route history document rejected its directly authored point");
            return;
        }
        this.phase = 4;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 4 {
        if !has_prim(this.doc, P0) { return; }
        let positioned = assembly_edit::transform(
            this.doc, "@root@", P0, [0.0, 1.0, -8.0], (), (),
            assembly_edit::describe(this.doc).generation,
        );
        if !query_ok(positioned) {
            fail(this, "route history document rejected its initial point pose");
            return;
        }
        this.phase = 5;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 5 {
        if !x_is(this.doc, P0, 0.0) { return; }
        this.checks.push(t_true(true,
            "route point is directly authored in the addressed USD document"));
        let moved = waypoint_editor::on_move(
            route_context(this.doc, P0, [3.0, 1.0, -8.0]),
        );
        if !query_ok(moved) || moved.path != P0 {
            fail(this, "route tool did not accept the point move");
            return;
        }
        this.phase = 6;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 6 {
        if !x_is(this.doc, P0, 3.0) { return; }
        this.checks.push(t_true(true,
            "route tool persists a point move in the addressed document"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if !query_ok(undone) {
            fail(this, "UndoDocument rejected the route-point move");
            return;
        }
        this.phase = 7;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 7 {
        if !x_is(this.doc, P0, 0.0) { return; }
        this.checks.push(t_true(true, "undo restores the route-point position"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if !query_ok(redone) {
            fail(this, "RedoDocument rejected the route-point move");
            return;
        }
        this.phase = 8;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 8 {
        if !x_is(this.doc, P0, 3.0) { return; }
        this.checks.push(t_true(true, "redo reapplies the route-point position"));
        let deleted = waypoint_editor::on_delete(
            route_context(this.doc, P0, [3.0, 1.0, -8.0]),
        );
        if !query_ok(deleted) || deleted.path != P0 {
            fail(this, "route tool did not accept point deletion");
            return;
        }
        this.phase = 9;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 9 {
        if has_prim(this.doc, P0) { return; }
        this.checks.push(t_true(true, "route tool removes the addressed point"));
        let undone = cmd("UndoDocument", #{ doc_id: this.doc });
        if !query_ok(undone) {
            fail(this, "UndoDocument rejected the route-point deletion");
            return;
        }
        this.phase = 10;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 10 {
        if !x_is(this.doc, P0, 3.0) { return; }
        this.checks.push(t_true(true,
            "undo restores the deleted route point and its pose"));
        let redone = cmd("RedoDocument", #{ doc_id: this.doc });
        if !query_ok(redone) {
            fail(this, "RedoDocument rejected the route-point deletion");
            return;
        }
        this.phase = 11;
        this.elapsed = 0.0;
        return;
    }

    if this.phase == 11 {
        if has_prim(this.doc, P0) { return; }
        this.checks.push(t_true(true, "redo reapplies the route-point deletion"));
        finish(this, this.checks);
    }
}

fn on_stop(me) {
    // Verdicts are emitted only from on_tick; teardown must not add one.
}
