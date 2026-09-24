// Explicit editor workflow coordination.
//
// This library joins the existing document owners for the common
// inspect -> apply -> projection -> lint -> save checkpoint. It does not own
// a document, queue a background watcher, or write a USD layer itself.
//
// Authored autosave is deliberately opt-in per Twin. The documented setting is
// `usd.editor_autosave`; an absent key is the safe disabled default. Runtime
// overlay persistence remains a separate `usd.runtime_persistence` setting.
import "assembly_edit" as edit;

fn editor_autosave_setting() {
    "usd.editor_autosave"
}

fn workflow_error(stage, message) {
    #{ ok: false, kind: "editor_workflow", stage: stage, error: message }
}

/// Normalize the setting value without confusing an omitted value with true.
/// This pure helper also gives Twin-local recipes a deterministic policy check.
fn autosave_policy_for(value) {
    let key = editor_autosave_setting();
    if value == () {
        return #{
            ok: true,
            key: key,
            enabled: false,
            explicit: false,
            source: "documented_default",
        };
    }
    if type_of(value) != "bool" {
        return #{
            ok: false,
            key: key,
            enabled: false,
            explicit: true,
            source: "twin_manifest",
            error: key + " must be a boolean",
        };
    }
    #{
        ok: true,
        key: key,
        enabled: value,
        explicit: true,
        source: "twin_manifest",
    }
}

/// Read the active Twin's authored editor-save policy.
fn autosave_policy() {
    autosave_policy_for(get_twin_setting(editor_autosave_setting()))
}

fn _checkpoint_result(stage, status, retryable, doc, generation, detail) {
    #{ok: status == "verified", kind: "editor_workflow",
      stage: stage, status: status, retryable: retryable,
      doc_id: doc, generation: generation, detail: detail}
}

/// Read back one exact Editor edit checkpoint without changing document,
/// preview, selection, camera, or perspective. `preview_id` and `view_id` are
/// optional only when the requested document is already the focused preview
/// and its focused view is unambiguous. A successful result binds one
/// generation to the current Editor projection, requested composed paths, and
/// lint report; screenshot capture remains a separate render command.
fn checkpoint(doc, expected_generation, affected_paths, preview_id, view_id) {
    if doc == () || affected_paths == () || type_of(affected_paths) != "array"
        || affected_paths.is_empty() {
        return _checkpoint_result("input", "invalid_input", false, doc,
            expected_generation, "checkpoint requires a document and non-empty exact paths");
    }
    let inspection = edit::describe(doc);
    if inspection == () || inspection.ok == false || inspection.generation == () {
        return _checkpoint_result("inspect", "inspection_failed", false, doc,
            expected_generation, inspection);
    }
    let generation = if expected_generation == () {
        inspection.generation
    } else {
        expected_generation
    };
    if inspection.generation != generation {
        return _checkpoint_result("generation", "stale", false, doc, generation,
            #{expected: generation, actual: inspection.generation});
    }

    let viewport = edit::viewport();
    if viewport == () || viewport.previews == () {
        return _checkpoint_result("viewport", "pending", true, doc, generation,
            "Editor preview inventory is not available yet");
    }
    let preview = ();
    for candidate in viewport.previews {
        let selected = if preview_id == () {
            candidate.focused == true && candidate.doc_id == doc
        } else {
            candidate.preview == preview_id && candidate.doc_id == doc
        };
        if selected { preview = candidate; }
    }
    if preview == () {
        return _checkpoint_result("viewport", "pending", true, doc, generation,
            #{requested_preview: preview_id, focused_preview: viewport.focused_preview});
    }
    if preview.projection_ready != true || preview.projected_generation != generation {
        return _checkpoint_result("projection", "pending", true, doc, generation,
            #{preview_id: preview.preview, projected_generation: preview.projected_generation,
              projection_ready: preview.projection_ready});
    }
    let selected_view = ();
    if preview.views != () {
        for candidate in preview.views {
            let selected = if view_id == () {
                candidate.focused == true
            } else {
                candidate.view == view_id
            };
            if selected { selected_view = candidate; }
        }
    }
    if selected_view == () {
        return _checkpoint_result("view", "pending", true, doc, generation,
            #{preview_id: preview.preview, requested_view: view_id});
    }

    let readback = query("QueryUsdPrims", #{doc_id: doc, paths: affected_paths});
    if readback == () || readback.ok == false || readback.records == ()
        || readback.records.len() != affected_paths.len() {
        return _checkpoint_result("readback", "readback_failed", false, doc,
            generation, readback);
    }
    for index in 0..affected_paths.len() {
        let record = readback.records[index];
        if record.path != affected_paths[index] {
            return _checkpoint_result("readback", "readback_failed", false, doc,
                generation, #{expected_path: affected_paths[index], record: record});
        }
        if record.generation != generation {
            return _checkpoint_result("generation", "stale", false, doc,
                generation, #{path: record.path, actual: record.generation});
        }
    }

    let lint_command = cmd("RunLint", #{
        domain: "usd", scope: "loaded_stages", policy: "warn", doc_id: doc,
    });
    if lint_command == () || lint_command.ok != true {
        return _checkpoint_result("lint", "lint_failed", false, doc, generation,
            lint_command);
    }
    let lint = query("LintReport", #{doc_id: doc});
    if lint == () || lint.ok == false {
        return _checkpoint_result("lint", "lint_failed", false, doc, generation,
            lint);
    }
    if lint.projection_ready != true || lint.generation != generation {
        return _checkpoint_result("lint", "pending", true, doc, generation,
            lint);
    }
    if lint.errors != () && lint.errors > 0 {
        return _checkpoint_result("lint", "lint_errors", false, doc, generation,
            lint);
    }

    let after = edit::describe(doc);
    if after == () || after.ok == false || after.generation != generation {
        return _checkpoint_result("generation", "stale", false, doc, generation,
            #{before: inspection.generation, after: if after == () { () } else { after.generation }});
    }
    #{ok: true, kind: "editor_workflow", stage: "verified", status: "verified",
      retryable: false, doc_id: doc, generation: generation,
      preview_id: preview.preview, view_id: selected_view.view,
      projection: #{ready: preview.projection_ready,
        generation: preview.projected_generation, mode: selected_view.mode,
        projection: selected_view.projection},
      paths: affected_paths, records: readback.records,
      lint: lint, save_required: after.dirty == true, dirty: after.dirty,
      screenshot: #{required: true, command: "CaptureScreenshot",
        preview_id: preview.preview, view_id: selected_view.view,
        document_generation: generation}}
}

/// Run the current-generation projection and document lint checkpoint.
///
/// A pending projection is a normal retryable result. It is never linted from
/// an older stage, and it never reaches SaveDocument.
fn verify(doc) {
    if doc == () {
        return workflow_error("inspect", "verify requires an explicit document id");
    }
    let inspection = edit::describe(doc);
    if inspection == () || inspection.ok == false {
        return workflow_error("inspect", "InspectUsdDocument failed");
    }

    let lint_command = cmd("RunLint", #{
        domain: "usd",
        scope: "loaded_stages",
        policy: "warn",
        doc_id: doc,
    });
    if lint_command == () || lint_command.ok != true {
        return workflow_error("lint", "RunLint could not be admitted");
    }
    let lint = query("LintReport", #{ doc_id: doc });
    if lint == () || lint.ok == false {
        return workflow_error("lint", "LintReport could not be read");
    }
    if lint.projection_ready != true || lint.generation != inspection.generation {
        return #{
            ok: false,
            kind: "editor_workflow",
            stage: "projection",
            retryable: true,
            doc_id: doc,
            generation: inspection.generation,
            inspection: inspection,
            lint: lint,
        };
    }
    if lint.errors != () && lint.errors > 0 {
        return #{
            ok: false,
            kind: "editor_workflow",
            stage: "lint",
            retryable: false,
            doc_id: doc,
            generation: inspection.generation,
            inspection: inspection,
            lint: lint,
        };
    }
    #{
        ok: true,
        kind: "editor_workflow",
        stage: "verified",
        retryable: false,
        doc_id: doc,
        generation: inspection.generation,
        inspection: inspection,
        lint: lint,
        saved: false,
    }
}

/// Verify an already applied edit, then save only when the Twin explicitly
/// enables `usd.editor_autosave`. Call this after one reviewed Apply/commit;
/// it is not a per-tick hook. A projection retry is safe to call again.
fn after_edit(doc) {
    let checked = verify(doc);
    if checked.ok != true { return checked; }

    let policy = autosave_policy();
    if policy.ok != true {
        checked.stage = "settings";
        checked.autosave = policy;
        checked.error = policy.error;
        return checked;
    }
    checked.autosave = policy;
    if !policy.enabled {
        checked.save_required = true;
        checked.note = "authored autosave is disabled; call assembly_edit::save_document explicitly";
        return checked;
    }

    let save = edit::save_document(doc);
    if save == () || save.ok != true {
        checked.stage = "save";
        checked.save = save;
        checked.error = "SaveDocument was not admitted";
        return checked;
    }
    let after = edit::describe(doc);
    checked.save = save;
    checked.after_save = after;
    checked.saved = after != () && after.ok == true && after.dirty == false;
    checked.stage = if checked.saved { "saved" } else { "save_pending" };
    checked.retryable = !checked.saved;
    checked
}

/// Commit one reviewed proposal, then run the same explicit checkpoint. If
/// projection is still settling, the result is retryable and no save occurs.
fn commit_and_after_edit(doc, proposal) {
    let committed = edit::commit_proposal(proposal);
    if committed == () || committed.ok != true {
        return #{
            ok: false,
            kind: "editor_workflow",
            stage: "apply",
            retryable: false,
            commit: committed,
        };
    }
    let checked = after_edit(doc);
    #{
        ok: checked.ok,
        kind: "editor_workflow",
        stage: checked.stage,
        retryable: checked.retryable,
        commit: committed,
        checkpoint: checked,
    }
}
