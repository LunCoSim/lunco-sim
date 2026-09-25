// Production coverage for the public USD document query providers.
const ROOT = "/UsdQueryApi";
const ASSEMBLY = ROOT + "/Assembly";
const ARC_TARGET = ROOT + "/Site/Route/W0";
const DELTA_TARGET = ASSEMBLY + "/DeltaPart";

fn finish(me, checks) {
    if me.done { return; }
    me.done = true;
    report_verdict(checks, "USD QUERY API", "USD_QUERY_API");
}

fn fail(me, message) {
    finish(me, [t_true(false, message)]);
}

fn find_document() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == () { return (); }
    for document in documents.open_documents {
        if document.kind != "usd" { continue; }
        let root = query("InspectUsdDocument", #{ doc_id: document.doc_id, path: ROOT });
        if root != () && root.ok != false && root.prim != () && root.prim.exists {
            return document.doc_id;
        }
    }
    ()
}

fn on_start(me, ctx) {
    this.done = false;
    this.elapsed = 0.0;
    this.phase = 0;
    this.doc = ();
    this.base_generation = -1;
    this.command_id = ();
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        fail(this, "USD query API did not reach a terminal verdict in phase " + this.phase);
        return;
    }

    if this.phase == 0 {
        this.doc = find_document();
        if this.doc == () { return; }

        let missing_id = query("InspectUsdDocument", #{});
        if missing_id == () || missing_id.ok != false
            || !missing_id.error.contains("explicit numeric `doc_id`") {
            fail(this, "InspectUsdDocument did not reject an omitted document identity");
            return;
        }

        let document = query("InspectUsdDocument", #{ doc_id: this.doc, path: ROOT + "/Assembly" });
        if document == () || document.ok == false || document.prim == ()
            || !document.prim.exists || document.prim.type != "Xform" {
            fail(this, "InspectUsdDocument did not return the authored assembly prim");
            return;
        }
        this.base_generation = document.generation;
        let fast_generation = usd_document_generation(this.doc);
        if fast_generation == () || fast_generation != this.base_generation {
            fail(this, "usd_document_generation did not accept the typed document id");
            return;
        }

        let arc = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: ARC_TARGET,
            edit_target: "@root@",
        });
        if arc == () || arc.ok == false {
            fail(this, "ResolveUsdTarget rejected the authored prim under a USD reference arc");
            return;
        }
        if arc.source != "canonical_stage" {
            return;
        }
        if arc.status != "resolved" || !arc.authored_here || !arc.under_arc
            || arc.edit_scope != "authored_layer" {
            fail(this, "ResolveUsdTarget lost the authored target's reference-arc scope");
            return;
        }

        let edit = assembly_edit::add_prim(
            this.doc,
            "@root@",
            ASSEMBLY,
            "DeltaPart",
            "Xform",
            (),
            this.base_generation,
        );
        if edit == () || edit.id == () || edit.ok == false {
            fail(this, "the public USD edit command did not accept the delta fixture operation");
            return;
        }
        this.command_id = edit.id;
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        let command = command_result(this.command_id);
        if command.status == "pending" { return; }
        if command.status != "applied" || command.ok != true {
            fail(this, "the delta fixture operation failed: " + command.error);
            return;
        }

        let resolved = query("ResolveUsdTarget", #{
            doc_id: this.doc,
            path: DELTA_TARGET,
            edit_target: "@root@",
        });
        if resolved == () || resolved.ok == false || resolved.status != "resolved"
            || !resolved.authored_here || !resolved.authored_in_document {
            fail(this, "ResolveUsdTarget did not resolve the newly authored local target");
            return;
        }

        let delta = query("SyncUsdDocument", #{
            doc_id: this.doc,
            since_generation: this.base_generation,
        });
        if delta == () || delta.ok == false || delta.kind != "delta"
            || delta.from_generation != this.base_generation
            || delta.ops.len() != 1 {
            fail(this, "SyncUsdDocument did not return the single-op generation delta");
            return;
        }
        let revision_step = delta.to_generation - delta.from_generation;
        if to_int(revision_step) != 1 {
            fail(this, "a single document operation did not advance one generation");
            return;
        }
        if to_int(delta.to_generation) != to_int(this.base_generation) + 1 {
            fail(this, "SyncUsdDocument did not advance the single-op delta by one");
            return;
        }
        if usd_document_generation(this.doc) != delta.to_generation {
            fail(this, "usd_document_generation did not advance with the committed document");
            return;
        }

        let future = query("SyncUsdDocument", #{
            doc_id: this.doc,
            since_generation: delta.to_generation + revision_step,
        });
        if future == () || future.ok != false || !future.error.contains("newer than document") {
            fail(this, "SyncUsdDocument did not reject a cursor newer than the document");
            return;
        }

        let operations = [];
        let expected_snapshot_generation = delta.to_generation;
        for index in 0..256 {
            operations.push(#{ AddPrim: #{
                edit_target: "@root@",
                parent_path: ASSEMBLY,
                name: "Part" + (index + 1).to_string(),
                type_name: "Xform",
                reference: (),
                reference_prim_path: "",
            }});
            expected_snapshot_generation += revision_step;
        }
        this.expected_snapshot_generation = expected_snapshot_generation;
        let batch = assembly_edit::batch(
            this.doc,
            "Exercise USD query history window",
            operations,
            delta.to_generation,
        );
        if batch == () || batch.id == () || batch.ok == false {
            fail(this, "the history-window fixture operations were rejected");
            return;
        }
        this.command_id = batch.id;
        this.phase = 2;
        return;
    }

    let command = command_result(this.command_id);
    if command.status == "pending" { return; }
    if command.status != "applied" || command.ok != true {
        fail(this, "history-window fixture operations failed: " + command.error);
        return;
    }

    let snapshot = query("SyncUsdDocument", #{
        doc_id: this.doc,
        since_generation: this.base_generation,
    });
    if snapshot == () || snapshot.ok == false || snapshot.kind != "snapshot"
        || snapshot.reason != "history_window_exceeded"
        || snapshot.generation != this.expected_snapshot_generation
        || !snapshot.layers.root.source.contains("Part256") {
        fail(this, "SyncUsdDocument did not return a complete snapshot after history expired");
        return;
    }

    finish(this, [
        t_true(true, "document inspection is explicit and reads composed USD facts"),
        t_true(true, "reference-arc target resolution uses the mounted OpenUSD stage"),
        t_true(true, "document synchronization returns typed deltas and rejects future cursors"),
        t_true(true, "expired operation history returns the complete authored layer snapshot"),
    ]);
}
