// TEST_KIND is intentionally omitted: this exercises the production document,
// projection, lint, and save command path without a second editor runtime.

const PART = "/EditorWorkflowTest/Part";

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "EDITOR WORKFLOW", "EDITOR_WORKFLOW");
    }
}

fn find_usd_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" && item.active == true
            && item.origin != () && item.origin.kind == "file"
            && item.origin.path.ends_with("editor_workflow.usda") {
            return item.doc_id;
        }
    }
    -1
}

fn on_start(me) {
    this.doc = -1;
    this.phase = "discover";
    this.elapsed = 0.0;
    this.done = false;
    this.checks = [];
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 30.0 {
        fail(this, "editor workflow did not reach a terminal checkpoint");
        return;
    }

    if this.doc < 0 {
        this.doc = find_usd_document();
        if this.doc < 0 { return; }
    }

    if this.phase == "discover" {
        let before = assembly_edit::describe(this.doc);
        if before == () || before.ok == false || before.generation == () || before.dirty != false {
            fail(this, "fixture document entered dirty before the authored edit");
            return;
        }
        this.before_generation = before.generation;
        let edit = assembly_edit::transform(
            this.doc, "@root@", PART, [1.0, 0.0, 0.0], (), (), before.generation,
        );
        if edit == () || edit.ok != true {
            fail(this, "typed apply did not admit the fixture transform");
            return;
        }
        this.phase = "checkpoint";
        return;
    }

    if this.phase == "checkpoint" {
        let checked = editor_workflow::after_edit(this.doc);
        if checked.stage == "projection" { return; }
        if checked.ok != true {
            fail(this, "explicit inspect/projection/lint checkpoint failed: " + checked.error);
            return;
        }
        this.checks.push(t_true(
            checked.autosave.ok == true && checked.autosave.enabled == false
                && checked.autosave.explicit == false,
            "missing Twin autosave setting did not resolve to the disabled default",
        ));
        this.checks.push(t_true(
            checked.saved == false && checked.save_required == true,
            "editor workflow wrote a file even though authored autosave was disabled",
        ));
        this.checks.push(t_true(
            checked.inspection.dirty == true && checked.lint.projection_ready == true
                && checked.generation > this.before_generation,
            "checkpoint did not preserve the changed generation and dirty authored state after lint",
        ));

        let missing = editor_workflow::autosave_policy_for(());
        let enabled = editor_workflow::autosave_policy_for(true);
        let invalid = editor_workflow::autosave_policy_for(1);
        this.checks.push(t_true(
            missing.enabled == false && enabled.enabled == true && invalid.ok == false,
            "autosave policy did not distinguish omitted, enabled, and malformed values",
        ));
        this.done = true;
        report_verdict(this.checks, "EDITOR WORKFLOW", "EDITOR_WORKFLOW");
    }
}
