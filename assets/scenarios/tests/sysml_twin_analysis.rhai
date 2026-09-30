// Production contract for Twin-scoped asynchronous SysML source-set analysis.
// The scene-test host waits for the initial Twin analysis, then exercises a
// saved document edit through asset reload and restores the checked-in source.

const PATH = "twin://sysml_async_analysis";
const SOURCE_PATH = "twin://sysml_async_analysis/analysis.sysml";
const REFRESH_MARKER = "\n// saved-source refresh acceptance probe\n";
const REFRESH_TIMEOUT_TICKS = 600;

fn simulation_dependencies(me, ctx) {
    #{
        modelica_entities: [],
        entity_reads: [], entity_writes: [],
        query_reads: ["AnalyzeSysml", "InspectSysmlDocument", "ListOpenDocuments"],
        required_inputs: [
            #{ owner: "sysml.twin-analysis", identity: twin_name() },
        ],
    }
}

fn initial_analysis_checks(analyzed) {
    let checks = [];
    let policy = policy_status();
    let twin_presentation = invoke_hook("camera.default_presentation", [#{}]);
    checks.push(t_true(policy.scope.contains("sysml_async_analysis")
        && policy.error == ()
        && policy.failed.len() == 0
        && policy.required_failures.len() == 0
        && policy.installed == ["camera.default_presentation"]
        && policy.lifecycle.event == "assets_mounted"
        && policy.lifecycle.status == "ok"
        && twin_presentation.ok == true
        && twin_presentation.value == "none",
        "the indexed Twin policy manifest installs its scoped hook without replacing the application lifecycle"));

    let usage = ();
    let unrelated_usage = ();
    let verification = ();
    let requirement_coverage = ();
    if analyzed != () && analyzed.analysis != () {
        for requirement in analyzed.analysis.requirements {
            if requirement.element.kind == "RequirementUsage"
                && requirement.element.qualified_name == "AsyncTwinAnalysis::powerBudget" {
                usage = requirement;
            }
            if requirement.element.kind == "RequirementUsage"
                && requirement.element.qualified_name == "AsyncTwinAnalysis::otherBudget" {
                unrelated_usage = requirement;
            }
        }
        for candidate in analyzed.analysis.verifications {
            if candidate.element.qualified_name == "AsyncTwinAnalysis::VerifyPowerBudget" {
                verification = candidate;
            }
        }
        requirement_coverage = sysml_requirement_coverage(
            analyzed.analysis.verifications);
    }
    checks.push(t_true(analyzed != () && analyzed.ok == true
        && analyzed.analysis != ()
        && analyzed.analysis.source_files.len() == 1
        && analyzed.analysis.source_files[0] == SOURCE_PATH
        && analyzed.analysis.source_revision != ()
        && usage != (),
        "the Twin query returns the committed source-set snapshot with its authored requirement usage"));
    let covers_resolved_target = false;
    let excludes_unrelated_usage = false;
    if analyzed != () && analyzed.analysis != () && usage != ()
        && unrelated_usage != () && verification != ()
        && requirement_coverage != () {
        let coverage = requirement_coverage;
        if !verification.verified_requirements.is_empty() {
            covers_resolved_target = coverage.covers(
                verification.element.handle, verification.verified_requirements[0]);
        }
        excludes_unrelated_usage = !coverage.covers(
            verification.element.handle, unrelated_usage.element.handle);
    }
    checks.push(t_true(verification != () && verification.verifies.len() == 1
        && verification.verifies[0] == "powerBudget",
        "the verification projection retains its authored verify objective"));
    checks.push(t_true(verification != () && verification.verified_requirements.len() == 1,
        "the verification projection publishes one resolved requirement handle"));
    checks.push(t_true(covers_resolved_target,
        "verification coverage includes its resolved target handle"));
    checks.push(t_true(excludes_unrelated_usage,
        "verification coverage excludes the other same-snapshot requirement"));

    let unmounted = query("AnalyzeSysml", #{
        path: "twin://sysml_async_analysis_missing",
        tables: ["requirements"],
    });
    checks.push(t_true(unmounted != () && unmounted.ok == false
        && unmounted.errors.len() > 0,
        "an unmounted Twin returns an explicit analysis error"));
    checks
}

fn inspect_source_document() {
    let documents = query("ListOpenDocuments");
    if documents == () || documents.open_documents == () {
        return #{ ok: false, error: "ListOpenDocuments did not return its documented result" };
    }
    let selected = ();
    let matches = 0;
    for document in documents.open_documents {
        if document.kind == "sysml" && document.origin != ()
            && document.origin.kind == "file" && document.origin.path != ()
            && document.origin.writable == true
            && document.origin.path.contains("sysml_async_analysis")
            && document.origin.path.ends_with("analysis.sysml") {
            matches += 1;
            selected = document;
        }
    }
    if matches != 1 {
        return #{ ok: false,
            error: "expected exactly one open Twin source document, found " + matches.to_string() };
    }
    #{ ok: true, doc_id: selected.doc_id, path: selected.origin.path }
}

fn analyze_twin() {
    try {
        query("AnalyzeSysml", #{
            path: PATH,
            tables: ["requirements", "verifications"],
        })
    } catch (error) { () }
}

fn begin_restore(doc_id, original_source) {
    // Return the restore result; the scenario hook owns its phase and verdict state.
    if doc_id == () || original_source == () {
        return #{ ok: false, error: "source document identity or baseline source is unavailable" };
    }
    let inspection = ();
    try { inspection = query("InspectSysmlDocument", #{ doc_id: doc_id }); }
    catch (error) { }
    if inspection == () || inspection.source == () || inspection.generation == () {
        return #{ ok: false, error: "could not inspect source document during restoration" };
    }

    if inspection.source != original_source {
        let restored = ();
        try {
            restored = cmd("ApplySysmlOps", #{
                doc_id: doc_id,
                parent_generation: inspection.generation,
                ops: [#{ ReplaceSource: #{ source: original_source } }],
            });
        } catch (error) { }
        if restored == () || restored.ok != true {
            return #{ ok: false, error: "could not restore the source document" };
        }
    }

    let saved = ();
    try { saved = cmd("SaveSysmlDocument", #{ doc_id: doc_id }); }
    catch (error) { }
    if saved == () || saved.ok != true {
        return #{ ok: false, error: "could not save the restored source document" };
    }
    #{ ok: true }
}

fn finish(checks, failure) {
    if failure != "" {
        checks.push(t_true(false, failure));
    }
    report_verdict(checks, "SYSML TWIN ANALYSIS", "SYSML_TWIN_ANALYSIS");
}

fn on_start(me, ctx) {
    this.done = false;
    this.phase = 0;
    this.phase_started_tick = sim_tick();
    this.wait_ticks = 0;
    this.checks = [];
    this.failure = "";
    this.doc_id = ();
    this.original_source = ();
    this.source_captured = false;
    this.document_ready_check_added = false;
    this.baseline_revision = ();
    this.saved_revision = ();
    this.restore_save_ok = false;
    this.restore_complete = false;
    this.refresh_pending_seen = false;
    this.stale_ready_after_pending = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.wait_ticks += 1;

    if this.phase == 0 {
        let inspection = ();
        if this.baseline_revision == () {
            let analyzed = analyze_twin();
            if analyzed == () || analyzed.ok != true || analyzed.analysis == () {
                if this.wait_ticks > REFRESH_TIMEOUT_TICKS {
                    this.failure = "initial Twin SysML analysis did not become ready";
                    finish(this.checks, this.failure);
                    this.done = true;
                }
                return;
            }

            this.checks = initial_analysis_checks(analyzed);
            this.baseline_revision = analyzed.analysis.source_revision;
            let document = inspect_source_document();
            if document.ok != true {
                this.failure = document.error;
                finish(this.checks, this.failure);
                this.done = true;
                return;
            }
            this.doc_id = document.doc_id;
            inspection = query("InspectSysmlDocument", #{ doc_id: this.doc_id });
            if inspection == () || inspection.generation == () || inspection.source == ()
                || inspection.dirty != false
                || !document.path.ends_with("analysis.sysml") {
                this.failure = "the indexed Twin source document was not open and clean";
                finish(this.checks, this.failure);
                this.done = true;
                return;
            }
            this.original_source = inspection.source;
            this.source_captured = true;
            if this.original_source.contains(REFRESH_MARKER) {
                this.failure = "the source fixture contains a leftover refresh probe marker";
                finish(this.checks, this.failure);
                this.done = true;
                return;
            }
        } else {
            inspection = query("InspectSysmlDocument", #{ doc_id: this.doc_id });
        }

        if inspection == () || inspection.generation == () || inspection.source == () {
            this.failure = "the indexed Twin source document could not be inspected";
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }
        if !this.source_captured || inspection.source != this.original_source
            || inspection.dirty != false {
            this.failure = "the indexed Twin source changed while its document analysis was preparing";
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }
        if inspection.analysis_state == "failed" {
            this.failure = "the indexed Twin document analysis failed before the refresh probe";
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }
        if inspection.analysis_state != "ready"
            || inspection.analysis_generation != inspection.generation {
            if this.wait_ticks > REFRESH_TIMEOUT_TICKS {
                this.failure = "the indexed Twin document analysis did not become ready";
                finish(this.checks, this.failure);
                this.done = true;
            }
            return;
        }
        if !this.document_ready_check_added {
            this.checks.push(t_true(inspection.analysis_generation == inspection.generation,
                "the writable Twin source document is backed by its current document analysis"));
            this.document_ready_check_added = true;
        }

        let current_analysis = analyze_twin();
        if current_analysis == () || current_analysis.ok != true
            || current_analysis.analysis == () {
            if this.wait_ticks > REFRESH_TIMEOUT_TICKS {
                this.failure = "the mounted Twin analysis stopped being Ready before the source edit";
                finish(this.checks, this.failure);
                this.done = true;
            }
            return;
        }
        if current_analysis.analysis.source_revision != this.baseline_revision {
            this.failure = "the mounted Twin source revision changed before the source edit";
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }

        let edit = ();
        try {
            edit = cmd("ApplySysmlOps", #{
                doc_id: this.doc_id,
                parent_generation: inspection.generation,
                ops: [#{ EditText: #{
                    range_start: this.original_source.len(),
                    range_end: this.original_source.len(),
                    replacement: REFRESH_MARKER,
                } }],
            });
        } catch (error) { }
        if edit == () || edit.ok != true {
            this.failure = "the source probe edit was rejected";
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }
        let edited = query("InspectSysmlDocument", #{ doc_id: this.doc_id });
        this.checks.push(t_true(edited != ()
            && edited.generation > inspection.generation
            && edited.source == this.original_source + REFRESH_MARKER
            && edited.dirty == true,
            "editing SysML advances the document generation without saving the Twin source"));
        if edited == () || edited.generation == ()
            || edited.source != this.original_source + REFRESH_MARKER
            || edited.dirty != true {
            this.failure = "the source probe edit did not produce the expected document state";
            let restored = begin_restore(this.doc_id, this.original_source);
            if restored.ok == true {
                this.restore_save_ok = true;
                this.phase = 2;
                this.phase_started_tick = sim_tick();
            } else {
                this.failure += "; " + restored.error;
                finish(this.checks, this.failure);
                this.done = true;
            }
            return;
        }

        let before_save = analyze_twin();
        let unsaved_isolated = before_save != () && before_save.ok == true
            && before_save.analysis != ()
            && before_save.analysis.source_revision == this.baseline_revision;
        this.checks.push(t_true(unsaved_isolated,
            "an unsaved editor generation does not change the mounted Twin analysis"));
        if !unsaved_isolated {
            this.failure = "the mounted Twin analysis changed before the document was saved";
            let restored = begin_restore(this.doc_id, this.original_source);
            if restored.ok == true {
                this.restore_save_ok = true;
                this.phase = 2;
                this.phase_started_tick = sim_tick();
            } else {
                this.failure += "; " + restored.error;
                finish(this.checks, this.failure);
                this.done = true;
            }
            return;
        }

        let saved = ();
        try { saved = cmd("SaveSysmlDocument", #{ doc_id: this.doc_id }); }
        catch (error) { }
        let save_acknowledged = saved != () && saved.ok == true
            && saved.data != () && saved.data.generation == edited.generation;
        this.checks.push(t_true(save_acknowledged,
            "SaveSysmlDocument acknowledges the edited document generation"));
        if !save_acknowledged {
            this.failure = "SaveSysmlDocument did not acknowledge the edited generation";
            let restored = begin_restore(this.doc_id, this.original_source);
            if restored.ok == true {
                this.restore_save_ok = true;
                this.phase = 2;
                this.phase_started_tick = sim_tick();
            } else {
                this.failure += "; " + restored.error;
                finish(this.checks, this.failure);
                this.done = true;
            }
            return;
        }

        this.phase = 1;
        this.phase_started_tick = sim_tick();
        return;
    }

    if this.phase == 1 {
        let analyzed = analyze_twin();
        let refresh_pending = analyzed != () && analyzed.ok == false
            && analyzed.errors != () && analyzed.errors.len() > 0
            && analyzed.errors[0].contains("preparing asynchronously");
        if refresh_pending {
            this.refresh_pending_seen = true;
        }
        if this.refresh_pending_seen && analyzed != () && analyzed.ok == true
            && analyzed.analysis != ()
            && analyzed.analysis.source_revision == this.baseline_revision {
            this.stale_ready_after_pending = true;
        }
        if analyzed != () && analyzed.ok == true && analyzed.analysis != () {
            let revision = analyzed.analysis.source_revision;
            if revision != this.baseline_revision {
                this.checks.push(t_true(!this.stale_ready_after_pending,
                    "the previous Twin revision is not published again after refresh reports Pending"));
                this.saved_revision = revision;
                this.checks.push(t_true(analyzed.analysis.source_files.len() == 1
                    && analyzed.analysis.source_files[0] == SOURCE_PATH,
                    "the reloaded Twin analysis retains its indexed source identity"));
                this.checks.push(t_true(revision != this.baseline_revision,
                    "saving the edited source publishes a new Twin analysis revision"));
                let restored = begin_restore(this.doc_id, this.original_source);
                if restored.ok == true {
                    this.restore_save_ok = true;
                    this.phase = 2;
                    this.phase_started_tick = sim_tick();
                } else {
                    this.failure = "could not restore the source after observing the saved revision: "
                        + restored.error;
                    finish(this.checks, this.failure);
                    this.done = true;
                }
                return;
            }
        }
        if sim_tick() - this.phase_started_tick > REFRESH_TIMEOUT_TICKS {
            this.failure = "saved SysML source did not produce a new Twin analysis revision";
            let restored = begin_restore(this.doc_id, this.original_source);
            if restored.ok == true {
                this.restore_save_ok = true;
                this.phase = 2;
                this.phase_started_tick = sim_tick();
            } else {
                this.failure += "; " + restored.error;
                finish(this.checks, this.failure);
                this.done = true;
            }
        }
        return;
    }

    if this.phase == 2 {
        let analyzed = analyze_twin();
        let twin_restored = analyzed != () && analyzed.ok == true
            && analyzed.analysis != ()
            && analyzed.analysis.source_revision == this.baseline_revision;
        let inspection = query("InspectSysmlDocument", #{ doc_id: this.doc_id });
        let document_restored = inspection != ()
            && inspection.source == this.original_source
            && inspection.dirty == false
            && inspection.analysis_state == "ready"
            && inspection.analysis_generation == inspection.generation;
        if twin_restored && document_restored {
            this.checks.push(t_true(this.restore_save_ok && document_restored,
                "the original source is saved and its document analysis is ready"));
            this.checks.push(t_true(this.saved_revision != ()
                && this.saved_revision != this.baseline_revision
                && twin_restored,
                "the Twin analysis returns to the original source revision after restoration"));
            this.restore_complete = document_restored && this.restore_save_ok;
            finish(this.checks, this.failure);
            this.done = true;
            return;
        }
        if sim_tick() - this.phase_started_tick > REFRESH_TIMEOUT_TICKS {
            this.checks.push(t_true(this.restore_save_ok && document_restored,
                "the original source is saved and its document analysis is ready"));
            this.checks.push(t_true(twin_restored,
                "the Twin analysis returns to the original source revision after restoration"));
            this.failure += "; restored source analysis did not become ready at both owners";
            finish(this.checks, this.failure);
            this.done = true;
        }
        return;
    }

    this.failure = "the SysML analysis gate entered an unknown phase";
    finish(this.checks, this.failure);
    this.done = true;
}

fn on_stop(me, ctx) {
    if this.restore_complete || this.doc_id == () || this.original_source == () { return; }
    let inspection = ();
    try { inspection = query("InspectSysmlDocument", #{ doc_id: this.doc_id }); }
    catch (error) { }
    if inspection != () && inspection.generation != ()
        && (inspection.source != this.original_source || inspection.dirty == true) {
        let restored_ok = true;
        if inspection.source != this.original_source {
            let restored = ();
            try {
                restored = cmd("ApplySysmlOps", #{
                    doc_id: this.doc_id,
                    parent_generation: inspection.generation,
                    ops: [#{ ReplaceSource: #{ source: this.original_source } }],
                });
            } catch (error) { }
            restored_ok = restored != () && restored.ok == true;
        }
        if restored_ok {
            try { cmd("SaveSysmlDocument", #{ doc_id: this.doc_id }); }
            catch (error) { }
        }
    }
}
