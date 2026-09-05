// Positive production acceptance test for the authored assembly workflow.
// Rhai discovers the catalog entry, selects a compatible advertised socket,
// submits the public attach command, inspects the composed relationships,
// saves/reloads the document, and exercises document undo/redo.

const ROOT = "/AssemblyWorkflowTest";
const HOST = ROOT + "/Chassis";
const SOCKET_KIND = "probe";
const JOINT_KIND = "fixed";
const CATALOG_ASSET = "components/mounting/demo_probe.usda";
// Keep the SaveAs artifact under Cargo's ignored output tree. The scenario
// still exercises the real file-backed save/reload path without dirtying the
// repository after every test run.
const SAVED_PATH = "target/scene-tests/assembly_workflow_saved.usda";
const COMPONENT = HOST + "/Payload";
const JOINT = HOST + "/assembly_joint";

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "ASSEMBLY WORKFLOW", "ASSEMBLY_WORKFLOW");
    }
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.doc = -1;
    this.synced = false;
    this.phase = "discover";
    this.asset = "";
    this.socket = "";
    this.done = false;
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn one_relationship(record, name) {
    if !query_ok(record) || record.relationships == () || record.relationships[name] == () {
        return ();
    }
    record.relationships[name]
}

fn array_has(values, expected) {
    if values == () { return false; }
    for value in values {
        if value == expected { return true; }
    }
    false
}

fn prim_exists(doc, path) {
    let inspection = assembly_edit::inspect(doc, path);
    query_ok(inspection) && inspection.prim != () && inspection.prim.exists == true
}

fn find_usd_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" { return item.doc_id; }
    }
    -1
}

fn document_is_saved_copy(doc) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return false; }
    for item in docs.open_documents {
        if item.doc_id == doc && item.origin != () && item.origin.kind == "file"
            && item.origin.path.contains(SAVED_PATH) {
            return true;
        }
    }
    false
}

fn document_is_dirty(doc) {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return (); }
    for item in docs.open_documents {
        if item.doc_id == doc { return item.dirty; }
    }
    ()
}

fn catalog_entry() {
    let catalog = query("ListSpawnCatalog");
    if catalog == () || catalog.entries == () { return (); }
    for entry in catalog.entries {
        if entry.source != () && entry.source.kind == "usd_file"
            && entry.source.path == CATALOG_ASSET {
            return entry;
        }
    }
    ()
}

fn compatible_socket(asset, require_empty) {
    let host = query("QueryUsdPrim", #{
        path: HOST,
        rels: ["lunco:mount:sockets"],
    });
    let sockets = one_relationship(host, "lunco:mount:sockets");
    if sockets == () { return (); }

    // Compatibility is authored data: schema, accepted plug kind, joint kind,
    // matching asset, and empty occupancy. The script does not infer a path
    // from a grouping name or from the component/joint leaf names.
    for socket_path in sockets {
        let socket = query("QueryUsdPrim", #{
            path: socket_path,
            attrs: [
                "lunco:mount:socket",
                "lunco:mount:joint",
                "lunco:mount:asset",
            ],
            rels: ["lunco:mount:part"],
        });
        if !query_ok(socket) || socket.attrs == () { continue; }
        let occupied = one_relationship(socket, "lunco:mount:part");
        if socket.attrs["lunco:mount:socket"] == SOCKET_KIND
            && socket.attrs["lunco:mount:joint"] == JOINT_KIND
            && socket.attrs["lunco:mount:asset"] == asset
            && (!require_empty || occupied == () || occupied.len() == 0) {
            return socket_path;
        }
    }
    ()
}

fn attached_relationships(me) {
    let component = assembly_edit::inspect(me.doc, COMPONENT);
    let part = query("QueryUsdPrim", #{
        path: COMPONENT,
        rels: ["lunco:mount:attachmentJoint"],
    });
    let socket = query("QueryUsdPrim", #{
        path: me.socket,
        rels: ["lunco:mount:part"],
    });
    let host = query("QueryUsdPrim", #{
        path: HOST,
        rels: ["lunco:mount:sockets"],
        children: true,
    });
    let part_joint = one_relationship(part, "lunco:mount:attachmentJoint");
    let socket_part = one_relationship(socket, "lunco:mount:part");
    let host_sockets = one_relationship(host, "lunco:mount:sockets");
    if !query_ok(component) || component.prim == () || component.prim.exists != true
        || !query_ok(part) || !query_ok(socket) || !query_ok(host)
        || part_joint == () || socket_part == () || host_sockets == ()
        || part_joint.len() != 1 || socket_part.len() != 1
        || part_joint[0] != JOINT || socket_part[0] != COMPONENT
        || !array_has(host_sockets, me.socket)
        || host.children == () || !array_has(host.children, COMPONENT)
        || !array_has(host.children, JOINT) {
        return false;
    }
    assembly_audit::mount_contract_report((), [#{
        host: HOST,
        socket: me.socket,
        component: COMPONENT,
        joint: JOINT,
        socket_kind: SOCKET_KIND,
        joint_kind: JOINT_KIND,
        asset: me.asset,
    }]).ok
}

fn detached_relationships(me) {
    let socket = query("QueryUsdPrim", #{
        path: me.socket,
        rels: ["lunco:mount:part"],
    });
    let occupied = one_relationship(socket, "lunco:mount:part");
    !prim_exists(me.doc, COMPONENT) && !prim_exists(me.doc, JOINT)
        && query_ok(socket) && occupied != () && occupied.len() == 0
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.doc < 0 {
        this.doc = find_usd_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "no open USD document was discoverable"); }
            return;
        }

        if !this.synced {
            let snapshot = assembly_edit::sync_document(this.doc, ());
            if !query_ok(snapshot) || snapshot.kind != "snapshot" {
                fail(this, "assembly_edit sync_document did not return a document snapshot");
                return;
            }
            let description = assembly_edit::describe(this.doc);
            if !query_ok(description) || description.doc != this.doc {
                fail(this, "assembly_edit describe did not identify the open document");
                return;
            }
            this.synced = true;
        }
    }

    // A scene restart recompiles the authored scenario. The saved document
    // origin distinguishes that post-reload state from the initial scene and
    // also lets an undo-triggered re-projection continue without re-attaching.
    if this.phase == "discover" && document_is_saved_copy(this.doc) {
        let entry = catalog_entry();
        if entry == () {
            if this.elapsed > 12.0 { fail(this, "saved workflow could not restore its socket selection"); }
            return;
        }
        this.asset = "lunco://" + entry.source.path;
        this.socket = compatible_socket(this.asset, false);
        if this.socket == () {
            if this.elapsed > 12.0 { fail(this, "saved workflow could not restore its socket selection"); }
            return;
        }
        let dirty = document_is_dirty(this.doc);
        if dirty == () {
            if this.elapsed > 12.0 { fail(this, "document dirty state was not discoverable after reload"); }
            return;
        }
        if prim_exists(this.doc, COMPONENT) {
            // SaveAs + RestartScene produces a clean saved document. Undo
            // makes that same document dirty again; after redo the component
            // is present while dirty, so the workflow can finish without
            // storing phase metadata in the document's own history.
            this.phase = if dirty { "check_redo" } else { "after_reload" };
        } else {
            this.phase = "check_undo";
        }
        this.elapsed = 0.0;
    } else if this.phase == "discover" && prim_exists(this.doc, COMPONENT) {
        this.phase = "check_attached";
        this.elapsed = 0.0;
    }

    if this.phase == "discover" {
        let entry = catalog_entry();
        if entry == () {
            if this.elapsed > 12.0 { fail(this, "catalog did not discover the mountable component"); }
            return;
        }
        this.asset = "lunco://" + entry.source.path;
        this.socket = compatible_socket(this.asset, true);
        if this.socket == () {
            if this.elapsed > 12.0 { fail(this, "no compatible empty socket was selectable"); }
            return;
        }

        // The attachment is authored in the persistent root layer so the
        // subsequent SaveAs + RestartScene proves the normal document path.
        let result = assembly_edit::attach_component(this.doc, #{
            edit_target: "@root@",
            socket_path: this.socket,
            host_path: HOST,
            name: "Payload",
            joint_name: "assembly_joint",
            asset: this.asset,
            placement: [0.0, 0.0, 0.0],
            rotate_deg: [0.0, 0.0, 0.0],
            joint: "Fixed",
        });
        if !result.ok {
            fail(this, "AttachComponent was rejected: " + result.error);
            return;
        }
        this.phase = "check_attached";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "check_attached" {
        if this.elapsed < 1.0 { return; }
        if !attached_relationships(this) {
            if this.elapsed < 12.0 { return; }
            fail(this, "attach relationship inspection failed");
            return;
        }

        let save = cmd("SaveAsDocument", #{
            doc: this.doc,
            path: SAVED_PATH,
        });
        if !save.ok {
            fail(this, "SaveAsDocument was rejected: " + save.error);
            return;
        }
        // SaveAs is synchronous; restart is the explicit production reload
        // boundary and keeps the document's undo history intact.
        let reload = cmd("RestartScene", #{ reset_document: false });
        if !reload.ok {
            fail(this, "RestartScene was rejected: " + reload.error);
            return;
        }
        this.phase = "after_reload";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "after_reload" {
        if this.elapsed < 1.0 { return; }
        if !document_is_saved_copy(this.doc) {
            fail(this, "reload did not retain the saved document origin");
            return;
        }
        if document_is_dirty(this.doc) != false {
            fail(this, "reload did not restore a clean saved document");
            return;
        }
        if !attached_relationships(this) {
            if this.elapsed < 12.0 { return; }
            fail(this, "saved attachment did not survive reload");
            return;
        }
        let undo = assembly_edit::undo(this.doc);
        if !undo.ok {
            fail(this, "UndoDocument was rejected: " + undo.error);
            return;
        }
        this.phase = "check_undo";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "check_undo" {
        if this.elapsed < 1.0 { return; }
        if !detached_relationships(this) {
            if this.elapsed < 12.0 { return; }
            fail(this, "one undo did not remove the component, joint, and occupancy");
            return;
        }
        let redo = assembly_edit::redo(this.doc);
        if !redo.ok {
            fail(this, "RedoDocument was rejected: " + redo.error);
            return;
        }
        this.phase = "check_redo";
        this.elapsed = 0.0;
        return;
    }

    if this.phase == "check_redo" {
        if this.elapsed < 1.0 { return; }
        if !attached_relationships(this) {
            if this.elapsed < 12.0 { return; }
            fail(this, "redo did not restore the complete attachment");
            return;
        }
        this.done = true;
        report_verdict([
            t_true(true, "catalog discovery and compatible socket selection"),
            t_true(true, "attach and relationship inspection"),
            t_true(true, "save and reload"),
            t_true(true, "atomic undo and redo"),
        ], "ASSEMBLY WORKFLOW", "ASSEMBLY_WORKFLOW");
    }
}
