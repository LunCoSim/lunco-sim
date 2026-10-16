// Production Rhai acceptance for USD document authoring history.
// The test creates an untitled USD document, edits one authored point through
// the public document tools, and observes undo/redo through the public query
// surface. Mounted waypoint-editor behavior is covered by route_lifecycle.

const ROOT = "/World/History";
const ROUTE = ROOT + "/Route";
const P0 = ROUTE + "/P0";
const TRIGGER = P0 + "/Trigger";
const P1 = ROUTE + "/P1";
const HISTORY_ATTR = "custom:historyMarker";
const SCENE_ROOT = "/RouteAuthoringHistory";
const PENDING_POINT = SCENE_ROOT + "/PendingHistoryPoint";

fn query_ok(value) {
    value != () && value.ok != false
}

fn find_new_usd_document(initial) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return (); }
    for item in docs.open_documents {
        if item.kind == "usd" && !initial.contains(item.doc_id) {
            return item.doc_id;
        }
    }
    ()
}

fn active_scene_document() {
    let scene = query("QueryUsdPrim", #{ path: SCENE_ROOT });
    if !query_ok(scene) || scene.doc_id == () { return (); }
    scene.doc_id
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

fn attribute_value(doc, path, name) {
    let record = query("QueryUsdPrim", #{ doc_id: doc, path: path, attrs: [name] });
    if !query_ok(record) || record.attrs == () { return (); }
    record.attrs[name]
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
    // The document scaffold uses metres; SetTranslate accepts canonical metres.
    let prefix = "(" + expected.to_string() + ",";
    position.starts_with(prefix)
}

fn finish(state, checks) {
    state.done = true;
    report_verdict(checks, "ROUTE AUTHORING HISTORY", "ROUTE_AUTHORING_HISTORY");
    state
}

fn fail(state, message) {
    finish(state, [t_true(false, message)])
}

fn test_state_is_valid(me) { this.failure == "" }
fn new_document_ready(me) { find_new_usd_document(this.initial_docs) != () }
fn history_root_ready(me) { has_prim(this.doc, ROOT) }
fn route_roots_ready(me) { has_prim(this.doc, ROUTE) }
fn route_points_ready(me) { has_prim(this.doc, P0) && has_prim(this.doc, P1) }
fn point_at_origin(me) { x_is(this.doc, P0, 0.0) }
fn point_moved(me) { x_is(this.doc, P0, 3.0) }
fn point_deleted(me) { !has_prim(this.doc, P0) }
fn point_restored(me) { has_prim(this.doc, P0) && x_is(this.doc, P0, 3.0) }
fn attribute_restored(me) {
    attribute_value(this.doc, ROOT, HISTORY_ATTR) == "tracked"
}
fn attribute_removed(me) { attribute_value(this.doc, ROOT, HISTORY_ATTR) == () }

fn pending_reference_restored(me) {
    let trigger = query("QueryUsdPrim", #{
        path: PENDING_POINT + "/Trigger",
        attrs: ["lunco:triggerZone"],
    });
    query_ok(trigger) && trigger.attrs != ()
        && trigger.attrs["lunco:triggerZone"] == "history_pending"
}

fn apply_step(state, action) {
    if action == "start" {
        let docs = query("ListOpenDocuments");
        if docs == () || docs.open_documents == () {
            state.failure = "open document list was unavailable";
            return state;
        }
        state.initial_docs = [];
        let has_usd_document = false;
        for item in docs.open_documents {
            if item.kind == "usd" {
                state.initial_docs.push(item.doc_id);
                has_usd_document = true;
            }
        }
        if !has_usd_document {
            state.failure = "the route history scene has no open USD document";
            return state;
        }
        state.scene_doc = active_scene_document();
        if state.scene_doc == () {
            state.failure = "the mounted history scene document was not readable";
            return state;
        }
        let created = cmd("NewDocument", #{ kind: "usd" });
        if !query_ok(created) {
            state.failure = "NewDocument did not admit an untitled USD document";
            return state;
        }
        state.waiting_for = "new USD document";
        return state;
    }

    if action == "capture_document" {
        let new_document = find_new_usd_document(state.initial_docs);
        if new_document == () {
            state.failure = "new USD document was not published";
            return state;
        }
        state.doc = new_document;
        let created = assembly_edit::add_prim(
            state.doc, "@root@", "/World", "History", "Scope", (),
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(created) {
            state.failure = "route history document rejected its root scope";
            return state;
        }
        state.waiting_for = "history root projection";
        return state;
    }

    if action == "create_route_roots" {
        let created = assembly_edit::add_prim(
            state.doc, "@root@", ROOT, "Route", "Scope", (),
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(created) {
            state.failure = "route history document rejected its route scope";
            return state;
        }
        state.waiting_for = "route scope";
        return state;
    }

    if action == "create_route_points" {
        let generation = assembly_edit::describe(state.doc).generation;
        let created = assembly_edit::batch(state.doc, "Create route history points", [
            #{ AddPrim: #{
                edit_target: "@runtime@", parent_path: ROUTE, name: "P0",
                type_name: "Xform", reference: (), reference_prim_path: "",
            }},
            #{ AddPrim: #{
                edit_target: "@runtime@", parent_path: ROUTE, name: "P1",
                type_name: "Xform", reference: (), reference_prim_path: "",
            }},
            #{ AddPrim: #{
                edit_target: "@runtime@", parent_path: P0, name: "Trigger",
                type_name: "Scope", reference: (), reference_prim_path: "",
            }},
            #{ SetAttribute: #{
                edit_target: "@runtime@", path: TRIGGER,
                name: "lunco:triggerZone", type_name: "token",
                value: "\"route_P0\"",
            }},
        ], generation);
        if !query_ok(created) {
            state.failure = "route history document rejected the atomic point subtree plan";
            return state;
        }
        state.waiting_for = "route point projection";
        return state;
    }

    if action == "verify_trigger" {
        let trigger = prim(state.doc, TRIGGER);
        if trigger == () {
            state.failure = "trigger child was not present in the USD document";
        } else {
            let value = attribute_value(state.doc, TRIGGER, "lunco:triggerZone");
            if value == () {
                state.failure = "trigger child has no readable lunco:triggerZone opinion; "
                    + "records=" + trigger.attributes.to_string();
            } else if value != "route_P0" {
                state.failure = "trigger identity was authored as " + value.to_string();
            }
        }
        return state;
    }

    if action == "set_initial_pose" {
        let positioned = assembly_edit::transform(
            state.doc, "@runtime@", P0, [0.0, 1.0, -8.0], (), (),
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(positioned) {
            state.failure = "route history document rejected its initial point pose";
            return state;
        }
        state.waiting_for = "initial point pose";
        return state;
    }

    if action == "move_point" {
        let moved = assembly_edit::transform(
            state.doc, "@runtime@", P0, [3.0, 1.0, -8.0], (), (),
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(moved) {
            state.failure = "USD document rejected the point move";
            return state;
        }
        state.waiting_for = "route point move";
        return state;
    }

    if action == "undo_move" || action == "redo_move"
        || action == "undo_delete" || action == "redo_delete"
        || action == "undo_attribute" || action == "redo_attribute" {
        let command = if action.starts_with("undo") { "UndoDocument" } else { "RedoDocument" };
        let result = cmd(command, #{ doc_id: state.doc });
        if !query_ok(result) {
            state.failure = command + " rejected " + action;
            return state;
        }
        state.waiting_for = action;
        return state;
    }

    if action == "delete_point" {
        let route = prim(state.doc, ROUTE);
        if route == () || route.children == () || route.children.len() != 2
            || !route.children.contains(P0) || !route.children.contains(P1) {
            state.failure = "route sibling order was not observable before deletion";
            return state;
        }
        state.route_children_before_delete = route.children;
        let deleted = assembly_edit::remove_prim(
            state.doc, "@runtime@", P0,
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(deleted) {
            state.failure = "USD document rejected point deletion";
            return state;
        }
        state.waiting_for = "route point deletion";
        return state;
    }

    if action == "add_attribute" {
        let added = assembly_edit::attribute(
            state.doc, "@root@", ROOT, HISTORY_ATTR, "string", "tracked",
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(added) {
            state.failure = "USD document rejected the history attribute";
            return state;
        }
        state.waiting_for = "history attribute authoring";
        return state;
    }

    if action == "remove_attribute" {
        let removed = assembly_edit::remove_attribute(
            state.doc, "@root@", ROOT, HISTORY_ATTR,
            assembly_edit::describe(state.doc).generation,
        );
        if !query_ok(removed) {
            state.failure = "USD document rejected the history attribute removal";
            return state;
        }
        state.waiting_for = "history attribute removal";
        return state;
    }

    if action == "restore_pending_reference" {
        let generation = assembly_edit::describe(state.scene_doc).generation;
        let authored = assembly_edit::batch(
            state.scene_doc, "Exercise pending referenced-root history", [
                #{ AddPrim: #{
                    edit_target: "@runtime@",
                    parent_path: SCENE_ROOT,
                    name: "PendingHistoryPoint",
                    type_name: "Xform",
                    reference: "lunco://markers/route_point.usda",
                    reference_prim_path: "/RoutePoint",
                }},
                #{ SetTranslate: #{
                    edit_target: "@runtime@",
                    path: PENDING_POINT,
                    value: [4.0, 1.0, -8.0],
                }},
                #{ SetAttribute: #{
                    edit_target: "@runtime@",
                    path: PENDING_POINT + "/Trigger",
                    name: "lunco:triggerZone",
                    type_name: "token",
                    value: "\"history_pending\"",
                }},
            ], generation,
        );
        if !query_ok(authored) && (authored == () || authored.status != "pending") {
            state.failure = "the mounted Twin rejected the referenced-root plan";
            return state;
        }
        state.pending_edit = authored;
        state.waiting_for = "referenced-root command completion";
        return state;
    }

    if action == "remove_and_restore_pending_reference" {
        let removed = assembly_edit::remove_prim(
            state.scene_doc, "@runtime@", PENDING_POINT,
            assembly_edit::describe(state.scene_doc).generation,
        );
        if !query_ok(removed) {
            state.failure = "the mounted Twin rejected pending-reference deletion";
            return state;
        }
        let restored = cmd("UndoDocument", #{ doc_id: state.scene_doc });
        if !query_ok(restored) {
            state.failure = "the mounted Twin rejected pending-reference undo";
            return state;
        }
        state.waiting_for = "pending referenced subtree projection";
        return state;
    }

    if action == "check_dome_command" {
        let path = ROOT + "/TestSky";
        let yaw = 0.123456789012345;
        let applied = cmd("SetDomeLight", #{
            doc_id: state.doc, path: path, intensity: 0.75,
            rotation: [0.0, yaw, 0.0],
        });
        let rotation = attribute_value(state.doc, path, "xformOp:rotateXYZ");
        let generation = assembly_edit::describe(state.doc).generation;
        let rejected = cmd("SetDomeLight", #{
            doc_id: state.doc, path: "relative/Sky", intensity: 9.0,
        });
        state.dome_verified = applied != () && applied.ok == true
            && attribute_value(state.doc, path, "inputs:intensity") == 0.75
            && rotation != () && rotation.len() == 3 && rotation[1] == yaw
            && rejected != () && rejected.ok == false && rejected.status == "rejected"
            && rejected.error != ()
            && assembly_edit::describe(state.doc).generation == generation;
        if !state.dome_verified {
            state.failure = "dome command precision or terminal rejection failed: applied="
                + applied.to_string() + "; rotation=" + rotation.to_string()
                + "; rejected=" + rejected.to_string();
        }
        return state;
    }

    if action == "invalidate_pending_discard" {
        let pending = cmd("DiscardDocument", #{doc_id: state.scene_doc});
        if pending == () || pending.status != "pending" || pending.id == () {
            state.failure = "file discard did not expose a pending command ticket: " + pending.to_string();
            return state;
        }
        state.discard_ticket = pending.id;
        let edited = assembly_edit::attribute(state.scene_doc, "@runtime@", SCENE_ROOT + "/TestLight",
            "userProperties:discardLeaseProbe", "string", "newer",
            assembly_edit::describe(state.scene_doc).generation);
        if edited == () || edited.ok != true {
            state.failure = "the newer edit was rejected while discard was reading: " + edited.to_string();
        }
        state.waiting_for = "stale discard rejection";
        return state;
    }

    if action == "cancel_pending_reference" {
        let result = assembly_edit::batch(state.doc, "Cancelled reference preparation", [
            #{ AddPrim: #{
                edit_target: "@runtime@", parent_path: ROOT, name: "CancelledPoint",
                type_name: "Xform", reference: "lunco://markers/route_point.usda",
                reference_prim_path: "/RoutePoint",
            }},
        ], assembly_edit::describe(state.doc).generation);
        if result == () || result.status != "pending" || result.id == () {
            state.failure = "cold reference edit did not expose a pending command ticket";
            return state;
        }
        state.cancel_ticket = result.id;
        cmd("CloseDocument", #{ doc_id: state.doc });
        state.waiting_for = "closed-document edit rejection";
        return state;
    }

    if action == "finish" {
        state.done = true;
        report_verdict([
            t_true(true, "document authoring persists a point move in the addressed USD document"),
            t_true(true, "point movement undo and redo preserve the authored pose"),
            t_true(true, "point deletion undo restores the subtree, pose, and sibling order"),
            t_true(true, "point deletion redo removes the restored subtree"),
            t_true(true, "attribute undo restores the exact value and redo removes it"),
            t_true(true, "undo restores a referenced subtree while its asset is pending"),
            t_true(state.dome_verified, "dome rotation retains double precision and invalid paths reject without mutation"),
            t_true(state.stale_discard_rejected, "a pending discard cannot overwrite a newer document revision"),
            t_true(state.unmounted_edit_verified, "an unmounted file prepares its existing dependency closure before an atomic child override"),
            t_true(state.closed_discard_rejected, "closing the addressed document rejects and retires its pending discard"),
            t_true(state.cancelled_edit_rejected, "closing the exact document rejects its pending dependency edit"),
        ], "ROUTE AUTHORING HISTORY", "ROUTE_AUTHORING_HISTORY");
        return state;
    }

    state.failure = "unknown route history task action " + action;
    state
}

fn task_start(me) { this = apply_step(this, "start"); }
fn task_capture_document(me) { this = apply_step(this, "capture_document"); }
fn task_create_route_roots(me) { this = apply_step(this, "create_route_roots"); }
fn task_create_route_points(me) { this = apply_step(this, "create_route_points"); }
fn task_verify_trigger(me) { this = apply_step(this, "verify_trigger"); }
fn task_set_initial_pose(me) { this = apply_step(this, "set_initial_pose"); }
fn task_move_point(me) { this = apply_step(this, "move_point"); }
fn task_undo_move(me) { this = apply_step(this, "undo_move"); }
fn task_redo_move(me) { this = apply_step(this, "redo_move"); }
fn task_delete_point(me) { this = apply_step(this, "delete_point"); }
fn task_undo_delete(me) { this = apply_step(this, "undo_delete"); }
fn task_redo_delete(me) { this = apply_step(this, "redo_delete"); }
fn task_verify_restored_order(me) {
    let doc = this.doc;
    let route = prim(doc, ROUTE);
    let children = if route == () || route.children == () {
        "<unavailable>"
    } else {
        route.children.to_string()
    };
    let trigger = attribute_value(doc, TRIGGER, "lunco:triggerZone");
    let expected = this.route_children_before_delete;
    let order_matches = route != () && route.children != ()
        && expected.len() == 2 && route.children.len() == 2
        && route.children[0] == expected[0] && route.children[1] == expected[1];
    let trigger_matches = trigger == "route_P0";
    if order_matches && trigger_matches { return; }
    this.failure = "restored subtree details: expected children=" + expected.to_string()
        + "; actual children=" + children
        + "; trigger="
        + if trigger == () { "<missing>" } else { trigger.to_string() };
}
fn task_add_attribute(me) { this = apply_step(this, "add_attribute"); }
fn task_remove_attribute(me) { this = apply_step(this, "remove_attribute"); }
fn task_undo_attribute(me) { this = apply_step(this, "undo_attribute"); }
fn task_redo_attribute(me) { this = apply_step(this, "redo_attribute"); }
fn task_restore_pending_reference(me) { this = apply_step(this, "restore_pending_reference"); }
fn referenced_edit_complete(me) {
    let result = this.pending_edit;
    if result.status == "pending" { result = command_result(result.id); }
    if result == () || result.status == "pending" { return false; }
    if result.ok != true { this.failure = "referenced-root plan failed: " + result.to_string(); }
    true
}
fn task_remove_and_restore_pending_reference(me) { this = apply_step(this, "remove_and_restore_pending_reference"); }
fn task_check_dome_command(me) { this = apply_step(this, "check_dome_command"); }
fn task_invalidate_pending_discard(me) { this = apply_step(this, "invalidate_pending_discard"); }
fn stale_discard_finished(me) {
    let result = command_result(this.discard_ticket);
    if result == () || result.status == "pending" { return false; }
    this.stale_discard_rejected = result.ok == false && result.status == "rejected"
        && result.error != () && pending_reference_restored(me)
        && attribute_value(this.scene_doc, SCENE_ROOT + "/TestLight", "userProperties:discardLeaseProbe") == "newer";
    if !this.stale_discard_rejected {
        this.failure = "stale discard did not preserve newer state: " + result.to_string();
    }
    true
}
fn task_cancel_pending_reference(me) { this = apply_step(this, "cancel_pending_reference"); }
fn cancelled_reference_finished(me) {
    let result = command_result(this.cancel_ticket);
    if result == () || result.status == "pending" { return false; }
    let closed = true;
    for document in query("ListOpenDocuments").open_documents {
        if document.doc_id == this.doc { closed = false; }
    }
    this.cancelled_edit_rejected = closed && result.ok == false
        && result.status == "rejected" && result.error != ();
    if !this.cancelled_edit_rejected {
        this.failure = "closed-document edit did not terminate with rejection: " + result.to_string();
    }
    true
}
fn task_open_source(me) {
    this.source_request = cmd("OpenUsdSourceDocument", #{source: "lunco://components/comms/antenna_tracking.usda"});
    this.waiting_for = "unmounted source document";
}
fn source_opened(me) {
    let result = this.source_request;
    if result.status == "pending" { result = command_result(result.id); }
    if result == () || result.status == "pending" { return false; }
    if result.ok != true || result.data == () || result.data.doc_id == () {
        this.failure = "source document failed to open: " + result.to_string();
    } else { this.source_doc = result.data.doc_id; }
    true
}
fn task_edit_unmounted_source(me) {
    this.source_request = assembly_edit::attribute(this.source_doc, "@runtime@",
        "/CommsAntenna/Mast", "userProperties:historyProbe", "string", "prepared",
        assembly_edit::describe(this.source_doc).generation);
    this.unmounted_was_pending = this.source_request != () && this.source_request.status == "pending";
    this.waiting_for = "unmounted referenced-prim edit";
}
fn unmounted_edit_finished(me) {
    let result = this.source_request;
    if result.status == "pending" { result = command_result(result.id); }
    if result == () || result.status == "pending" { return false; }
    this.unmounted_edit_verified = this.unmounted_was_pending && result.ok == true
        && attribute_value(this.source_doc, "/CommsAntenna/Mast", "userProperties:historyProbe") == "prepared";
    if !this.unmounted_edit_verified { this.failure = "unmounted dependency preparation failed: " + result.to_string(); }
    true
}
fn task_close_pending_discard(me) {
    let result = cmd("DiscardDocument", #{doc_id: this.source_doc});
    if result == () || result.status != "pending" || result.id == () {
        this.failure = "source discard did not return a pending ticket: " + result.to_string();
        return;
    }
    this.source_discard_ticket = result.id;
    cmd("CloseDocument", #{doc_id: this.source_doc});
    this.waiting_for = "closed-document discard rejection";
}
fn closed_discard_finished(me) {
    let result = command_result(this.source_discard_ticket);
    if result == () || result.status == "pending" { return false; }
    let closed = true;
    for document in query("ListOpenDocuments").open_documents {
        if document.doc_id == this.source_doc { closed = false; }
    }
    this.closed_discard_rejected = closed && result.ok == false && result.status == "rejected" && result.error != ();
    if !this.closed_discard_rejected { this.failure = "closed-document discard did not reject: " + result.to_string(); }
    true
}
fn task_finish(me) { this = apply_step(this, "finish"); }

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ListOpenDocuments", "QueryUsdPrim", "InspectUsdDocument"], required_inputs: [] }
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.initial_docs = [];
    this.route_children_before_delete = [];
    this.doc = ();
    this.scene_doc = ();
    this.pending_edit = ();
    this.cancel_ticket = ();
    this.discard_ticket = ();
    this.source_doc = ();
    this.source_request = ();
    this.source_discard_ticket = ();
    this.unmounted_was_pending = false;
    this.unmounted_edit_verified = false;
    this.closed_discard_rejected = false;
    this.dome_verified = false;
    this.stale_discard_rejected = false;
    this.cancelled_edit_rejected = false;
    this.failure = "";
    this.waiting_for = "route history task start";
}

fn task(me, ctx) {
    seq([
        once(Fn("task_start")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("new_document_ready")),
        once(Fn("task_capture_document")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("history_root_ready")),
        once(Fn("task_create_route_roots")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_roots_ready")),
        once(Fn("task_create_route_points")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("route_points_ready")),
        once(Fn("task_verify_trigger")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_set_initial_pose")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_at_origin")),
        once(Fn("task_move_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_moved")),
        once(Fn("task_undo_move")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_at_origin")),
        once(Fn("task_redo_move")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_moved")),
        once(Fn("task_delete_point")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_deleted")),
        once(Fn("task_undo_delete")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_restored")),
        once(Fn("task_verify_restored_order")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_redo_delete")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("point_deleted")),
        once(Fn("task_add_attribute")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("attribute_restored")),
        once(Fn("task_remove_attribute")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("attribute_removed")),
        once(Fn("task_undo_attribute")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("attribute_restored")),
        once(Fn("task_redo_attribute")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("attribute_removed")),
        once(Fn("task_restore_pending_reference")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("referenced_edit_complete")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_remove_and_restore_pending_reference")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("pending_reference_restored")),
        once(Fn("task_check_dome_command")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_invalidate_pending_discard")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("stale_discard_finished")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_open_source")),
        wait_until(Fn("source_opened")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_edit_unmounted_source")),
        wait_until(Fn("unmounted_edit_finished")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_close_pending_discard")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("closed_discard_finished")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_cancel_pending_reference")),
        check(Fn("test_state_is_valid")),
        wait_until(Fn("cancelled_reference_finished")),
        check(Fn("test_state_is_valid")),
        once(Fn("task_finish")),
    ])
}

fn on_event(me, evt, ctx) {
    if evt.name == "TASK_FAILED" && !this.done {
        let message = if this.failure == "" {
            "route authoring history task sequence failed while waiting for " + this.waiting_for
        } else {
            this.failure
        };
        this = fail(this, message);
    }
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 25.0 {
        this = fail(this, "route authoring history timed out waiting for " + this.waiting_for);
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this = fail(this, "route authoring history stopped while waiting for " + this.waiting_for);
    }
}
