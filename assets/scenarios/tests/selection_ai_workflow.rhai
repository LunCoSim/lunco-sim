// TEST-ONLY: selected-part agent policy over the production Editor owners.
const TEST_KIND = "graphics";
const PART = "/SelectionAiWorkflow/Part";
const OTHER = "/SelectionAiWorkflow/Other";

fn on_start(me) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.original = -1;
    this.fork = -1;
    this.initial_ids = [];
    this.checks = [];
}

fn ready(preview, doc, generation) {
    for p in assembly_edit::viewport().previews {
        if p.preview == preview && p.doc == doc && p.projection_ready
            && p.projected_generation == generation { return true; }
    }
    false
}

fn correction(context, generation) {
    assembly_edit::propose(context.doc, "Assembly", "Raise selected test part",
        [#{ SetTranslate: #{ edit_target: context.edit_target,
            path: context.primary.path, value: [0.0, 1.0, 0.0] } }], generation)
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 40.0 {
        this.checks.push(t_true(false, "selection workflow timeout phase " + this.phase));
        this.done = true;
        report_verdict(this.checks, "SELECTION AI WORKFLOW", "SELECTION_AI_WORKFLOW");
        return;
    }
    if this.phase == 0 {
        let docs = query("ListOpenDocuments");
        for record in docs.open_documents {
            this.initial_ids.push(record.doc_id);
            if record.kind == "usd" {
                let inspected = assembly_edit::inspect(record.doc_id, PART);
                if inspected.prim != () && inspected.prim.exists { this.original = record.doc_id; }
            }
        }
        if this.original < 0 { return; }
        assembly_edit::fork_document(this.original, "Selection workflow disposable fork");
        this.phase = 1;
        return;
    }
    if this.phase == 1 {
        for record in query("ListOpenDocuments").open_documents {
            if record.kind == "usd" && !this.initial_ids.contains(record.doc_id) {
                this.fork = record.doc_id;
            }
        }
        if this.fork < 0 { return; }
        assembly_ui::open_session(12201, this.original, "@root@");
        assembly_ui::open_session(12202, this.fork, "@root@");
        this.phase = 2;
        return;
    }
    if this.phase == 2 {
        if !ready(12201, this.original, 0) || !ready(12202, this.fork, 0) { return; }
        let empty = assembly_edit::selection_context();
        this.checks.push(t_true(empty.selection_state == "no_selection"
            && empty.requires_single_target, "no selection cannot imply a target"));
        assembly_ui::select_prim(12202, PART, false, false);
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let selected = assembly_edit::selection_context();
        if selected.primary == () || selected.primary.path != PART { return; }
        this.context = selected;
        this.checks.push(t_true(selected.doc == this.fork && selected.preview == 12202
            && selected.selection_mode == "single" && !selected.requires_single_target,
            "selected part has exact fork and preview identity"));
        let hidden = assembly_edit::selection_context_for(12201);
        this.checks.push(t_true(hidden.doc == this.original && !hidden.focused
            && hidden.selection_state == "no_selection", "hidden same-name document stays isolated"));
        let proposal = correction(selected, 0);
        let review = assembly_edit::review_session(selected.doc);
        if proposal.ok == false || review.proposals.len() != 1 {
            this.checks.push(t_true(false, "correction proposal was not created"));
            this.done = true;
            report_verdict(this.checks, "SELECTION AI WORKFLOW", "SELECTION_AI_WORKFLOW");
            return;
        }
        this.proposal = review.proposals[0].id;
        assembly_ui::select_prim(12202, OTHER, false, false);
        this.phase = 4;
        return;
    }
    if this.phase == 4 {
        let current = assembly_edit::selection_context();
        if current.primary == () || current.primary.path != OTHER { return; }
        this.checks.push(t_true(current.primary.identity != this.context.primary.identity,
            "selection changed during planning is detectable"));
        // Agent policy: never silently retarget or commit after context changes.
        assembly_edit::review_proposal(this.proposal, #{ Reject: () });
        this.checks.push(t_true(assembly_edit::describe(this.fork).generation == 0,
            "uncommitted proposal did not mutate either selected part"));
        assembly_ui::select_prim(12202, PART, true, false);
        this.phase = 5;
        return;
    }
    if this.phase == 5 {
        let multiple = assembly_edit::selection_context();
        if multiple.selected.len() != 2 { return; }
        this.checks.push(t_true(multiple.selection_mode == "multiple"
            && multiple.requires_single_target, "ambiguous singular request requires clarification"));
        assembly_ui::select_prim(12202, PART, false, false);
        this.phase = 6;
        return;
    }
    if this.phase == 6 {
        let current = assembly_edit::selection_context();
        if current.selected.len() != 1 || current.primary.path != PART { return; }
        let document = assembly_edit::describe(current.doc);
        let proposal = correction(current, document.generation);
        let session = assembly_edit::review_session(current.doc);
        if proposal.ok == false || session.proposals.len() != 1 {
            this.checks.push(t_true(false, "exact correction proposal is not reviewable"));
            this.done = true;
            report_verdict(this.checks, "SELECTION AI WORKFLOW", "SELECTION_AI_WORKFLOW");
            return;
        }
        this.checks.push(t_true(true, "proposal is reviewable before mutation"));
        assembly_edit::commit_proposal(session.proposals[0].id);
        this.phase = 7;
        return;
    }
    if this.phase == 7 {
        if !ready(12202, this.fork, 1) { return; }
        let changed = query("QueryUsdPrim", #{ doc: this.fork, path: PART });
        let untouched = query("QueryUsdPrim", #{ doc: this.original, path: PART });
        this.checks.push(t_true(changed.attrs["xformOp:translate"][1] == 1.0,
            "correction reached selected document projection"));
        this.checks.push(t_true(untouched.attrs["xformOp:translate"][1] == 0.0
            && assembly_edit::describe(this.original).generation == 0,
            "same-name part in other document is unchanged"));
        let stale = assembly_edit::batch(this.fork, "Reject stale correction",
            [#{ SetTranslate: #{ edit_target: "@root@", path: PART, value: [0.0, 9.0, 0.0] } }], 0);
        this.checks.push(t_true(stale.ok == false && assembly_edit::describe(this.fork).generation == 1,
            "stale generation fails without mutation"));
        cmd("UndoDocument", #{ doc: this.fork });
        this.phase = 8;
        return;
    }
    if this.phase == 8 {
        let generation = assembly_edit::describe(this.fork).generation;
        if generation <= 1 || !ready(12202, this.fork, generation) { return; }
        let restored = query("QueryUsdPrim", #{ doc: this.fork, path: PART });
        this.checks.push(t_true(restored.attrs["xformOp:translate"][1] == 0.0,
            "undo restores the selected part through the same projection"));
        assembly_edit::preview_close(12202);
        this.phase = 9;
        return;
    }
    if this.phase == 9 {
        let closed = assembly_edit::selection_context_for(12202);
        this.checks.push(t_true(closed.ok == false, "closed preview context is rejected"));
        assembly_edit::close_document(this.fork);
        assembly_edit::preview_close(12201);
        this.done = true;
        report_verdict(this.checks, "SELECTION AI WORKFLOW", "SELECTION_AI_WORKFLOW");
    }
}
