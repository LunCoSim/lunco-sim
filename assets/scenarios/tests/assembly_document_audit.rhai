// TEST-ONLY: run in the production windowed Editor to exercise preview leases.
const TEST_KIND = "graphics";
const ROOT = "/DocumentAudit";
const HINGE = ROOT + "/Hinge";

fn finish(checks) {
    report_verdict(checks, "ASSEMBLY DOCUMENT AUDIT", "ASSEMBLY_DOCUMENT_AUDIT");
}

fn on_start(me) {
    this.done = false;
    this.phase = 0;
    this.original = -1;
    this.fork = -1;
    this.elapsed = 0.0;
    this.initial_docs = [];
    this.checks = [];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.done = true;
        finish([t_true(false, "document audit timed out in phase " + this.phase)]);
        return;
    }
    if this.phase == 0 {
        let docs = query("ListOpenDocuments");
        if docs.open_documents == () { return; }
        this.initial_docs = [];
        for record in docs.open_documents { this.initial_docs.push(record.doc_id); }
        for record in docs.open_documents {
            if record.kind == "usd" {
                let prim = assembly_edit::inspect(record.doc_id, HINGE);
                if prim.ok != false && prim.prim != () && prim.prim.exists {
                    this.original = record.doc_id;
                    break;
                }
            }
        }
        if this.original < 0 { return; }
        assembly_edit::fork_document(this.original, "Audit fork");
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        let docs = query("ListOpenDocuments");
        for record in docs.open_documents {
            if record.kind == "usd" && !this.initial_docs.contains(record.doc_id) {
                this.fork = record.doc_id;
                break;
            }
        }
        if this.fork < 0 { return; }
        let unmapped = query("QueryUsdPrim", #{ doc: this.fork, path: HINGE });
        this.checks.push(t_true(unmapped.ok == false,
            "open document without a projection cannot read the live stage"));
        let before = assembly_edit::describe(this.fork);
        assembly_edit::attribute(this.fork, "@root@", HINGE,
            "physics:axis", "token", "\"Y\"", before.generation);
        assembly_edit::preview_open(15001, this.original, "@root@");
        assembly_ui::open_session(15002, this.fork, "@root@");
        this.phase = 2;
        return;
    }
    if this.phase == 4 {
        let docs = query("ListOpenDocuments");
        for record in docs.open_documents {
            if record.doc_id == this.fork { return; }
        }
        let closed = query("QueryUsdPrim", #{ doc: this.fork, path: HINGE });
        this.checks.push(t_true(closed.ok == false,
            "closed document cannot read a cached preview or the live stage"));
        let original = assembly_audit::joint_frame_report(this.original,
            [#{ path: HINGE, axis: "X" }]);
        this.checks.push(t_true(original.ok,
            "closing the fork leaves the original document query intact"));
        this.done = true;
        finish(this.checks);
        return;
    }
    if this.phase == 3 {
        let fork = query("QueryUsdPrim", #{ doc: this.fork, path: HINGE, attrs: ["physics:axis"] });
        if fork.ok == false { return; }
        this.checks.push(t_true(fork.attrs["physics:axis"] == "Z",
            "document query resumes with the newly synchronized generation"));
        assembly_edit::preview_close(15002);
        assembly_edit::close_document(this.fork);
        this.phase = 4;
        return;
    }
    let viewport = assembly_edit::viewport();
    let ready = 0;
    for preview in viewport.previews {
        if (preview.doc == this.original || preview.doc == this.fork)
            && preview.projection_ready { ready += 1; }
    }
    if ready != 2 { return; }
    let open_docs = query("ListOpenDocuments");
    this.checks.push(t_true(open_docs.open_documents.len() == this.initial_docs.len() + 1,
        "opening previews does not execute their copied scenario and create more forks"));
    let original = query("QueryUsdPrim", #{ doc: this.original, path: HINGE, attrs: ["physics:axis"] });
    let fork = query("QueryUsdPrim", #{ doc: this.fork, path: HINGE, attrs: ["physics:axis"] });
    let live = query("QueryUsdPrim", #{ path: HINGE, attrs: ["physics:axis"] });
    let missing = query("QueryUsdPrim", #{ doc: 999999999, path: HINGE });
    let checks = [
        t_true(original.ok != false && original.attrs["physics:axis"] == "X", "original document is X"),
        t_true(fork.ok != false && fork.attrs["physics:axis"] == "Y", "fork document is Y despite identical prim path"),
        t_true(live.ok != false && live.attrs["physics:axis"] == "X", "live query ignores preview copies"),
        t_true(missing.ok == false, "unknown document cannot fall through to the live scene"),
    ];
    for check in checks { this.checks.push(check); }
    let original_report = assembly_audit::joint_frame_report(this.original,
        [#{ path: HINGE, axis: "X" }]);
    let fork_report = assembly_audit::joint_frame_report(this.fork,
        [#{ path: HINGE, axis: "Y" }]);
    let wrong_report = assembly_audit::joint_frame_report(this.fork,
        [#{ path: HINGE, axis: "X" }]);
    this.checks.push(t_true(original_report.ok && fork_report.ok,
        "assembly audit helpers preserve each document's joint contract"));
    this.checks.push(t_true(!wrong_report.ok,
        "fork audit rejects the other document's expected axis"));
    assembly_edit::preview_focus(15001);
    let hidden_report = assembly_audit::joint_frame_report(this.fork,
        [#{ path: HINGE, axis: "Y" }]);
    this.checks.push(t_true(hidden_report.ok,
        "focusing the original leaves the hidden fork's query identity unchanged"));
    let position = assembly_audit::world_position(this.fork, ROOT + "/Part");
    this.checks.push(t_true(assembly_audit::close_vec(position, [11.0, 2.0, 3.0], 0.001),
        "preview position composes authored ancestors without a physics pose"));
    let malformed = query("QueryUsdPrim", #{ doc: "not-a-document", path: HINGE });
    this.checks.push(t_true(malformed.ok == false, "malformed document identity is rejected"));
    let before = assembly_edit::describe(this.fork);
    let edit = assembly_edit::attribute(this.fork, "@root@", HINGE,
        "physics:axis", "token", "\"Z\"", before.generation);
    let stale = query("QueryUsdPrim", #{ doc: this.fork, path: HINGE });
    this.checks.push(t_true(edit.ok && stale.ok == false,
        "an edit invalidates the document query until canonical synchronization"));
    this.phase = 3;
}
