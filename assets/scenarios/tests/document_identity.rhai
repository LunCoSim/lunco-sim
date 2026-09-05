// TEST-ONLY: mixed-domain document lifecycle through the production Editor.
const TEST_KIND = "graphics";

fn record_for(docs, id) {
    for record in docs.open_documents {
        if record.doc_id == id { return record; }
    }
    ()
}

fn on_start(me) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.original = -1;
    this.modelica = -1;
    this.fork = -1;
    this.initial_ids = [];
    this.checks = [];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        this.done = true;
        report_verdict([t_true(false, "document identity timed out in phase " + this.phase)],
            "DOCUMENT IDENTITY", "DOCUMENT_IDENTITY");
        return;
    }
    let docs = query("ListOpenDocuments");
    if this.phase == 0 {
        this.initial_ids = [];
        for record in docs.open_documents {
            this.initial_ids.push(record.doc_id);
            if record.kind == "usd" {
                let inspected = assembly_edit::inspect(record.doc_id, "/DocumentIdentity/Part");
                if inspected.prim != () && inspected.prim.exists {
                    this.original = record.doc_id;
                }
            }
        }
        if this.original < 0 { return; }
        cmd("NewDocument", #{ kind: "modelica" });
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        let original = record_for(docs, this.original);
        if original == () || original.kind != "usd" {
            this.done = true;
            report_verdict([t_true(false, "Modelica creation overwrote the USD workspace identity")],
                "DOCUMENT IDENTITY", "DOCUMENT_IDENTITY");
            return;
        }
        for record in docs.open_documents {
            if record.kind == "modelica" && !this.initial_ids.contains(record.doc_id) {
                this.modelica = record.doc_id;
            }
        }
        if this.modelica < 0 { return; }
        this.checks.push(t_true(this.modelica != this.original,
            "Modelica and USD have distinct live handles"));
        this.checks.push(t_true(original != () && original.kind == "usd",
            "Modelica creation preserves the USD workspace record"));
        this.checks.push(t_true(docs.open_documents.len() == this.initial_ids.len() + 1,
            "both domain records remain discoverable"));
        if this.modelica == this.original {
            this.done = true;
            report_verdict(this.checks, "DOCUMENT IDENTITY", "DOCUMENT_IDENTITY");
            return;
        }
        assembly_edit::fork_document(this.original, "Identity fork");
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        for record in docs.open_documents {
            if record.kind == "usd" && !this.initial_ids.contains(record.doc_id) {
                this.fork = record.doc_id;
            }
        }
        if this.fork < 0 { return; }
        this.checks.push(t_true(this.fork != this.modelica && this.fork != this.original,
            "fork allocation shares the cross-domain identity owner"));
        assembly_ui::open_session(15201, this.fork, "@root@");
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let ready = false;
        for preview in assembly_edit::viewport().previews {
            if preview.preview == 15201 && preview.doc == this.fork {
                ready = preview.projection_ready;
            }
        }
        if !ready { return; }
        let prim = query("QueryUsdPrim", #{ doc: this.fork, path: "/DocumentIdentity/Part" });
        this.checks.push(t_true(prim.ok != false, "fork preview resolves its USD document"));
        let wrong = assembly_edit::describe(this.modelica);
        this.checks.push(t_true(wrong.ok == false,
            "Modelica handle is rejected by the USD owner"));
        this.save_path = "target/document-identity-" + this.fork + ".usda";
        assembly_edit::save_as_document(this.fork, this.save_path);
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        let saved = record_for(docs, this.fork);
        if saved == () || saved.origin.kind != "file" { return; }
        let document = assembly_edit::describe(this.fork);
        this.checks.push(t_true(!document.dirty && saved.origin.path.ends_with(this.save_path),
            "Save-As persists only the addressed USD fork"));
        let modelica = record_for(docs, this.modelica);
        this.checks.push(t_true(modelica != () && modelica.kind == "modelica"
            && modelica.origin.kind == "untitled",
            "USD Save-As leaves Modelica origin unchanged"));
        let original = assembly_edit::describe(this.original);
        this.checks.push(t_true(!original.dirty && original.origin.File.path != saved.origin.path,
            "USD Save-As preserves the original file identity"));
        assembly_edit::preview_close(15201);
        assembly_edit::close_document(this.fork);
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        if record_for(docs, this.fork) != () { return; }
        this.checks.push(t_true(record_for(docs, this.modelica) != (),
            "closing USD fork preserves Modelica document"));
        this.checks.push(t_true(record_for(docs, this.original) != (),
            "closing USD fork preserves original document"));
        cmd("CloseDocument", #{ doc: this.modelica });
        this.phase = 5;
        return;
    }
    if record_for(docs, this.modelica) != () { return; }
    let original = assembly_edit::describe(this.original);
    this.checks.push(t_true(original.ok != false && record_for(docs, this.original) != (),
        "closing Modelica preserves the USD owner and workspace record"));
    this.done = true;
    report_verdict(this.checks, "DOCUMENT IDENTITY", "DOCUMENT_IDENTITY");
}
