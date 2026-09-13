// TEST-ONLY: selected-part agent policy over the production Editor owners.
const TEST_KIND = "editor";
const PART = "/SelectionAiWorkflow/Part";
const OTHER = "/SelectionAiWorkflow/Other";

fn has_operation(operations, name) {
    for operation in operations {
        if operation.operation == name { return true; }
    }
    false
}

fn on_start(me) {
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
    this.original = -1;
    this.fork = -1;
    this.initial_ids = [];
    this.checks = [];
    this.inspection_preset = "selection-workflow";
    this.preset_checked = false;
    this.view = -1;
}

fn ready(preview, doc) {
    let generation = assembly_edit::describe(doc).generation;
    let viewport = assembly_edit::viewport();
    for p in viewport.previews {
        if p.preview == preview && p.doc_id == doc && p.projection_ready
            && p.projected_generation == generation { return true; }
    }
    false
}

fn selection_doc(selection) {
    if selection.doc_id != () { selection.doc_id } else { selection.doc }
}

fn primary_view(preview, doc) {
    let viewport = assembly_edit::viewport();
    for p in viewport.previews {
        if p.preview == preview && p.doc_id == doc && p.views != () && p.views.len() > 0 {
            return p.views[0].view;
        }
    }
    -1
}

fn correction(context, generation) {
    assembly_edit::propose(selection_doc(context), "Assembly", "Raise selected test part",
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
        if !ready(12201, this.original) || !ready(12202, this.fork) { return; }
        this.view = primary_view(12202, this.fork);
        if this.view < 0 { return; }
        this.original_generation = assembly_edit::describe(this.original).generation;
        this.fork_generation = assembly_edit::describe(this.fork).generation;
        let saved = assembly_edit::save_inspection_preset(this.view, this.inspection_preset);
        this.checks.push(t_true(saved.ok != false,
            "inspection camera preset save is admitted through the typed view owner"));
        let empty = assembly_edit::selection_context();
        this.checks.push(t_true(empty.selection_state == "no_selection"
            && empty.requires_single_target, "no selection cannot imply a target"));
        let empty_authoring = assembly_builder::selected_authoring_context(());
        this.checks.push(t_true(!empty_authoring.ok,
            "selected authoring context rejects no selection"));
        assembly_ui::select_prim(12202, PART, false, false);
        this.phase = 3;
        return;
    }
    if this.phase == 3 {
        let selected = assembly_edit::selection_context();
        if selected.primary == () || selected.primary.path != PART { return; }
        this.context = selected;
        let presets = assembly_edit::inspection_presets();
        if presets.presets == () { return; }
        for preset in presets.presets {
            if preset.name == this.inspection_preset { this.preset_checked = true; }
        }
        if !this.preset_checked { return; }
        let applied = assembly_edit::apply_inspection_preset(this.view, this.inspection_preset);
        this.checks.push(t_true(applied.ok != false,
            "inspection camera preset applies to the addressed preview view"));
        this.checks.push(t_true(selection_doc(selected) == this.fork && selected.preview == 12202
            && selected.selection_mode == "single" && !selected.requires_single_target,
            "selected part has exact fork and preview identity"));
        let hidden = assembly_edit::selection_context_for(12201);
        this.checks.push(t_true(selection_doc(hidden) == this.original && !hidden.focused
            && hidden.selection_state == "no_selection", "hidden same-name document stays isolated"));
        let selected_authoring = assembly_builder::selected_authoring_context(12202);
        let selected_authoring_message = if selected_authoring.ok {
            "selected authoring context preserves exact identity and edit target"
        } else {
            "selected authoring context: " + selected_authoring.error
        };
        this.checks.push(t_true(selected_authoring.ok
                && selected_authoring.kind == "selected_authoring_context"
                && selected_authoring.path == PART
                && selected_authoring.selection_identity == selected.primary.identity
                && selected_authoring.edit_target == selected.edit_target,
            selected_authoring_message));
        this.checks.push(t_true(has_operation(
                selected.primary.supported_operations, "UpdateComponent"),
            "component selection advertises the generic recipe-driven update plan"));
        let component_context = component_editor::selected_update_context(12202, ());
        let component_context_message = if component_context.ok {
            "selected component context preserves the exact Editor checkpoint"
        } else {
            "selected component context: " + component_context.error
        };
        this.checks.push(t_true(component_context.ok
                && component_context.kind == "component_update_context"
                && component_context.path == PART
                && component_context.generation == selected_authoring.generation,
            component_context_message));
        let selected_doc = selection_doc(selected);
        let proposal = correction(selected, assembly_edit::describe(selected_doc).generation);
        let review = assembly_edit::review_session(selected_doc);
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
        this.checks.push(t_true(!has_operation(
                current.primary.supported_operations, "UpdateComponent"),
            "non-component selection does not advertise component regeneration"));
        // Agent policy: never silently retarget or commit after context changes.
        assembly_edit::review_proposal(this.proposal, #{ Reject: () });
        this.checks.push(t_true(assembly_edit::describe(this.fork).generation == this.fork_generation,
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
        let current_doc = selection_doc(current);
        let document = assembly_edit::describe(current_doc);
        let proposal = correction(current, document.generation);
        let session = assembly_edit::review_session(current_doc);
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
        if !ready(12202, this.fork) { return; }
        let committed_generation = assembly_edit::describe(this.fork).generation;
        let changed = query("QueryUsdPrim", #{ doc_id: this.fork, path: PART });
        let untouched = query("QueryUsdPrim", #{ doc_id: this.original, path: PART });
        let diff = authoring_inspection::candidate_diff(
            this.original, this.fork, [PART, "/SelectionAiWorkflow/RemovedPart"]);
        this.checks.push(t_true(diff.paths.len() == 2 && diff.paths[0].changed
            && !diff.paths[1].changed, "candidate diff preserves changed and absent paths"));
        let invalid_diff = authoring_inspection::candidate_diff(this.original, this.fork, []);
        this.checks.push(t_true(invalid_diff.ok == false,
            "candidate diff rejects an unbounded path request"));
        let groups = authoring_inspection::group_diagnostics([#{
            domain: "usd", rule: "missing_source", severity: "error",
            subject: PART, message: "test finding",
        }]);
        this.checks.push(t_true(groups.ok && groups.groups.len() == 1
            && groups.groups[0].owner == "Composition",
            "diagnostics group by owning domain and severity"));
        let snapshot = authoring_inspection::inspection_snapshot(
            this.fork, "/SelectionAiWorkflow", PART);
        let snapshot_message = if snapshot.ok == false {
            "inspection snapshot: " + snapshot.error
        } else if snapshot.path != PART {
            "inspection snapshot returned the wrong USD path"
        } else if snapshot.collision == () {
            "inspection snapshot omitted the collision surface"
        } else if snapshot.provenance == () {
            "inspection snapshot omitted the provenance surface"
        } else {
            "inspection snapshot combines composed geometry and provenance"
        };
        this.checks.push(t_true(snapshot.ok && snapshot.path == PART
            && snapshot.collision != () && snapshot.frames != ()
            && snapshot.materials != () && snapshot.provenance != (),
            snapshot_message));
        let inspection = authoring_inspection::inspection_mode(
            this.fork, "/SelectionAiWorkflow", PART, true);
        let inspection_off = authoring_inspection::inspection_mode(
            this.fork, "/SelectionAiWorkflow", PART, false);
        this.checks.push(t_true(inspection.ok && inspection.layers.ok
                && inspection_off.ok && inspection_off.layers.ok,
            "unified inspection mode controls reusable diagnostic layers"));
        let navigation = authoring_inspection::navigate_diagnostic(
            12202, this.view, #{ subject: PART });
        this.checks.push(t_true(navigation.ok && navigation.path == PART,
            "diagnostics navigate through canonical selection and preview framing"));
        this.checks.push(t_true(changed.attrs["xformOp:translate"][1] == 1.0,
            "correction reached selected document projection"));
        this.checks.push(t_true(untouched.attrs["xformOp:translate"][1] == 0.0
            && assembly_edit::describe(this.original).generation == this.original_generation,
            "same-name part in other document is unchanged"));
        let stale = assembly_edit::batch(this.fork, "Reject stale correction",
            [#{ SetTranslate: #{ edit_target: "@root@", path: PART, value: [0.0, 9.0, 0.0] } }], this.fork_generation);
        this.checks.push(t_true(stale.ok == false && assembly_edit::describe(this.fork).generation == committed_generation,
            "stale generation fails without mutation"));
        cmd("UndoDocument", #{ doc_id: this.fork });
        this.phase = 8;
        return;
    }
    if this.phase == 8 {
        let generation = assembly_edit::describe(this.fork).generation;
        if !ready(12202, this.fork) { return; }
        let restored = query("QueryUsdPrim", #{ doc_id: this.fork, path: PART });
        this.checks.push(t_true(restored.attrs["xformOp:translate"][1] == 0.0,
            "undo restores the selected part through the same projection"));
        let deleted = assembly_edit::delete_inspection_preset(this.inspection_preset);
        this.checks.push(t_true(deleted.ok != false,
            "inspection camera preset deletion is admitted through the typed settings owner"));
        assembly_edit::preview_close(12202);
        this.phase = 9;
        return;
    }
    if this.phase == 9 {
        let closed = assembly_edit::selection_context_for(12202);
        this.checks.push(t_true(closed.ok == false, "closed preview context is rejected"));
        let presets = assembly_edit::inspection_presets();
        let still_present = false;
        if presets.presets != () {
            for preset in presets.presets {
                if preset.name == this.inspection_preset { still_present = true; }
            }
        }
        this.checks.push(t_true(!still_present,
            "inspection camera preset deletion is reflected in persisted settings"));
        assembly_edit::close_document(this.fork);
        assembly_edit::preview_close(12201);
        this.done = true;
        report_verdict(this.checks, "SELECTION AI WORKFLOW", "SELECTION_AI_WORKFLOW");
    }
}
