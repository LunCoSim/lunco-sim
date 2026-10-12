// Production duplication gate. Use a UI-capable Modelica host with no active
// Twin, Application/Retain scenario lifetime, distinct root_a/root_b and the
// API-authored entry_scene present in both roots. The gate admits its saved
// Modelica package through the Twin source-root command. Error telemetry is
// part of the verdict.
import "lunco://scripting/tools/modelica_editor" as editor;

fn simulation_dependencies(me, params) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["ReadActiveTwinContract", "ListOpenDocuments", "GetDocumentSource", "CompileStatus", "RunStatus", "GetExperimentResult"],
      required_inputs: []}
}
fn record(title) {
    let listed = query("ListOpenDocuments", #{});
    if listed == () || listed.open_documents == () { return (); }
    for entry in listed.open_documents { if entry.kind == "modelica" && entry.title == title { return entry; } }
    ()
}
fn document_entry(doc) {
    let listed = query("ListOpenDocuments", #{});
    if listed == () || listed.open_documents == () { return (); }
    for entry in listed.open_documents { if entry.doc_id == doc { return entry; } }
    ()
}
fn ready(doc, class) {
    let status = query("CompileStatus", #{doc_id: doc});
    status != () && status.ok != false && status.ast_parsed == true
        && status.candidates != () && status.candidates.contains(class)
}
fn ack(result) {
    if result != () && result.status == "pending" && result.id != () { return command_result(result.id); }
    result
}
fn finish(me, failures) {
    if me.done { return me; }
    me.done = true;
    for doc in me.docs { editor::close(doc); }
    report_verdict(failures, "MODELICA DUPLICATE LIFECYCLE", "MODELICA_DUPLICATE_LIFECYCLE");
    me
}
fn request_copy(me, qualified, name, expected_title, expected_class, next) {
    me.copy_ack = cmd("OpenClass", #{qualified: qualified, action: #{Duplicate: #{name: name}}});
    me.expected_title = expected_title; me.expected_class = expected_class;
    me.next = next; me.phase = "copy";
    me
}
fn request_failure(me, qualified, name, diagnostic, next) {
    me.failure_before = me.errors.len(); me.failure_diagnostic = diagnostic;
    me.failure_title = name; me.next = next;
    me.copy_ack = cmd("OpenClass", #{qualified: qualified, action: #{Duplicate: #{name: name}}});
    me.phase = "failure";
    me
}
fn on_start(me, params) {
    this.done = false; this.ticks = 0; this.docs = []; this.errors = []; this.checks = [];
    this.root_a = params.root_a; this.root_b = params.root_b; this.entry = params.entry_scene;
    if this.root_a == () || this.root_b == () || this.entry == ()
        || this.root_a == "" || this.root_b == "" || this.entry == "" || this.root_a == this.root_b {
        this = finish(this, ["distinct fixture roots and an API-authored entry_scene are required"]); return;
    }
    let twin = query("ReadActiveTwinContract", #{});
    if twin == () || twin.open != false { this = finish(this, ["initial Application document scope is required: " + twin.to_string()]); return; }
    subscribe("modelica-duplicate-failed");
    subscribe("modelica-duplicate-cancelled");
    this.qualified = "DuplicateGateRoot.B.Part";
    this.quoted = "DuplicateGateRoot.B.'Quoted Part'";
    this.source_a = "package DuplicateGateRoot package A model Part Real fromA; end Part; end A; package B final /* α */ model Part Real fromB; end Part /*tail*/ ; model 'Quoted Part' Real quotedValue; end 'Quoted Part'; end B; end DuplicateGateRoot;";
    this.source_b = "package DuplicateGateRoot package A model Part Real successorA; end Part; end A; package B final /* α */ model Part Real successorB; end Part /*tail*/ ; model 'Quoted Part' Real quotedSuccessor; end 'Quoted Part'; end B; end DuplicateGateRoot;";
    let app = editor::new_scratch("package DuplicateGateRoot package B model Part Real fromApplication; end Part; end B; end DuplicateGateRoot;", "Duplicate application source");
    if app.ok != true { this = finish(this, [app.to_string()]); return; }
    this.app = app.doc_id; this.docs.push(this.app); this.phase = "application";
}
fn on_event(me, evt, ctx) {
    if this.done { return; }
    if evt.name == "modelica-duplicate-failed" || evt.name == "modelica-duplicate-cancelled" {
        this.errors.push(#{name: evt.name, message: evt.value.to_string()});
    }
}
fn on_tick(me, params) {
    if this.done { return; }
    this.ticks += 1;
    if this.ticks > 12000 { this = finish(this, ["duplicate lifecycle timed out at " + this.phase]); return; }
    if this.phase == "application" {
        if !ready(this.app, this.qualified) { return; }
        this.transition_ack = cmd("CreateTwin", #{path: this.root_a, name: "Duplicate owner A", default_scene: this.entry});
        this.phase = "owner_a"; return;
    }
    if this.phase == "owner_a" {
        let result = ack(this.transition_ack);
        if result != () && result.ok == false { this = finish(this, [result.to_string()]); return; }
        let twin = query("ReadActiveTwinContract", #{});
        if twin == () || twin.name != "Duplicate owner A" { return; }
        for title in ["DuplicateExactCopy", "'Quoted PartCopy'", "DuplicateMissingReuse", "DuplicateInvalidProbeCopy", "Bad Name", "DuplicateGateRootCopy", "DuplicateRetiredReuse"] {
            if record(title) != () { this = finish(this, ["fixture already owns reserved test title: " + title]); return; }
        }
        let source = editor::new_scratch(this.source_a, "Duplicate scoped source A");
        let invalid = editor::new_scratch("model DuplicateInvalidProbe Real ; end DuplicateInvalidProbe;", "DuplicateInvalidProbe");
        if source.ok != true || invalid.ok != true { this = finish(this, ["A source admission failed"]); return; }
        this.a = source.doc_id; this.invalid = invalid.doc_id;
        this.docs.push(this.a); this.docs.push(this.invalid); this.phase = "source_a"; return;
    }
    if this.phase == "source_a" {
        if !ready(this.a, this.qualified) || !ready(this.a, this.quoted) { return; }
        let entry = document_entry(this.a);
        if entry == () || entry.runtime_context.kind != "local_twin" {
            this = finish(this, ["A source did not receive its exact Twin owner: " + entry.to_string()]); return;
        }
        this.expected_owner = entry.runtime_context;
        this.marker = "fromB"; this.forbidden = "fromA";
        this = request_copy(this, this.qualified, "DuplicateExactCopy", "DuplicateExactCopy", "DuplicateGateRoot.B.DuplicateExactCopy", "quoted"); return;
    }
    if this.phase == "copy" {
        let result = ack(this.copy_ack);
        if result != () && result.ok == false { this = finish(this, ["duplicate dispatch rejected: " + result.to_string()]); return; }
        let entry = record(this.expected_title);
        if entry == () { return; }
        let source = query("GetDocumentSource", #{doc_id: entry.doc_id});
        if source == () || source.source == () || !ready(entry.doc_id, this.expected_title) { return; }
        if entry.runtime_context != this.expected_owner || source.origin.kind != "untitled"
            || !source.source.contains(this.marker) || source.source.contains(this.forbidden)
            || source.source.contains("fromApplication") || !source.source.contains("within DuplicateGateRoot.B;") {
            this = finish(this, ["duplicate published the wrong class bytes, lexical scope or runtime owner: " + source.to_string()]); return;
        }
        if (this.marker == "fromB" || this.marker == "successorB")
            && (!source.source.contains("final /* α */ model ") || !source.source.contains("/*tail*/ ;")) {
            this = finish(this, ["duplicate lost the complete parser-owned declaration extent: " + source.source]); return;
        }
        this.docs.push(entry.doc_id);
        this.checks.push(t_true(true, "installed exact qualified duplicate " + this.expected_class + " under its admitted owner"));
        this.phase = this.next; return;
    }
    if this.phase == "quoted" {
        this.marker = "quotedValue"; this.forbidden = "fromA";
        this = request_copy(this, this.quoted, "", "'Quoted PartCopy'", "DuplicateGateRoot.B.'Quoted PartCopy'", "absent"); return;
    }
    if this.phase == "absent" {
        this = request_failure(this, "DuplicateGateMissing.NoSuchClass", "DuplicateMissingReuse", "class source was not found", "reuse_missing"); return;
    }
    if this.phase == "failure" {
        let result = ack(this.copy_ack);
        if result != () && result.ok == false { this = finish(this, ["negative case never reached the preparation owner: " + result.to_string()]); return; }
        if this.errors.len() <= this.failure_before { return; }
        let error = this.errors[this.failure_before];
        if error.name != "modelica-duplicate-failed" || !error.message.contains(this.failure_diagnostic)
            || record(this.failure_title) != () {
            this = finish(this, ["failed duplicate missed its diagnostic or retained reserved document metadata: " + error.to_string()]); return;
        }
        this.checks.push(t_true(true, "preparation rejection released " + this.failure_title + " and emitted its owning error"));
        this.phase = this.next; return;
    }
    if this.phase == "reuse_missing" {
        this.marker = "fromB"; this.forbidden = "fromA";
        this = request_copy(this, this.qualified, "DuplicateMissingReuse", "DuplicateMissingReuse", "DuplicateGateRoot.B.DuplicateMissingReuse", "invalid_source"); return;
    }
    if this.phase == "invalid_source" {
        // Explicit source ID proves a selected malformed resident source error,
        // even though an invalid AST cannot participate in by-name lookup.
        this.failure_before = this.errors.len(); this.failure_diagnostic = "no valid selected class";
        this.failure_title = "DuplicateInvalidProbeCopy"; this.next = "reuse_invalid";
        this.copy_ack = cmd("DuplicateModelFromReadOnly", #{source_doc_id: this.invalid});
        this.phase = "failure"; return;
    }
    if this.phase == "reuse_invalid" {
        this = request_copy(this, this.qualified, "DuplicateInvalidProbeCopy", "DuplicateInvalidProbeCopy", "DuplicateGateRoot.B.DuplicateInvalidProbeCopy", "invalid_name"); return;
    }
    if this.phase == "invalid_name" {
        this = request_failure(this, this.qualified, "Bad Name", "invalid Modelica syntax", "close_pending_source"); return;
    }
    if this.phase == "close_pending_source" {
        // This producer allocates its task/reservation synchronously. The
        // source close occurs in this callback before the completion driver.
        this.copy_ack = cmd("DuplicateModelFromReadOnly", #{source_doc_id: this.a});
        let result = ack(this.copy_ack);
        if result == () || result.ok != true { this = finish(this, ["source-close duplicate was not admitted"]); return; }
        let pending = record("DuplicateGateRootCopy");
        if pending == () || pending.runtime_context != this.expected_owner {
            this = finish(this, ["source-close pending task was not observed; cancellation case unexercised"]); return;
        }
        let installed = query("GetDocumentSource", #{doc_id: pending.doc_id});
        if installed != () && installed.source != () {
            this.docs.push(pending.doc_id);
            this = finish(this, ["source-close copy already installed; pending cancellation case unexercised"]); return;
        }
        this.source_pending = pending.doc_id;
        this.cancel_before = this.errors.len();
        editor::close(this.a);
        this.phase = "source_cancelled"; return;
    }
    if this.phase == "source_cancelled" {
        if this.errors.len() <= this.cancel_before { return; }
        let error = this.errors[this.cancel_before];
        let installed = query("GetDocumentSource", #{doc_id: this.source_pending});
        if error.name != "modelica-duplicate-cancelled" || record("DuplicateGateRootCopy") != ()
            || (installed != () && installed.source != ()) {
            this = finish(this, ["source retirement did not cancel the observed task and release its reservation"]); return;
        }
        this.checks.push(t_true(true, "closing the exact source fenced observed pending task publication and released its namespace"));
        let replacement = editor::new_scratch(this.source_a, "Duplicate scoped source A replacement");
        if replacement.ok != true { this = finish(this, [replacement.to_string()]); return; }
        this.a = replacement.doc_id; this.docs.push(this.a); this.phase = "replacement_a"; return;
    }
    if this.phase == "replacement_a" {
        if !ready(this.a, this.qualified) { return; }
        this.marker = "fromB"; this.forbidden = "fromA";
        this = request_copy(this, this.qualified, "DuplicateGateRootCopy", "DuplicateGateRootCopy", "DuplicateGateRoot.B.DuplicateGateRootCopy", "replace_pending"); return;
    }
    if this.phase == "replace_pending" {
        this.copy_ack = cmd("OpenClass", #{qualified: this.qualified, action: #{Duplicate: #{name: "DuplicateRetiredReuse"}}});
        let result = ack(this.copy_ack);
        if result != () && result.ok == false { this = finish(this, ["original-target duplicate admission rejected"]); return; }
        // OpenClass can still have queued its World closure. An absent
        // reservation here is legitimate and never reinterpreted as success.
        let pending = record("DuplicateRetiredReuse");
        this.pending = ();
        if pending != () {
            if pending.runtime_context != this.expected_owner {
                this = finish(this, ["observed reservation did not preserve originating A"]); return;
            }
            this.pending = pending.doc_id;
        }
        this.transition_ack = cmd("CreateTwin", #{path: this.root_b, name: "Duplicate owner B", default_scene: this.entry});
        this.phase = "owner_b"; return;
    }
    if this.phase == "owner_b" {
        let result = ack(this.transition_ack);
        if result != () && result.ok == false { this = finish(this, [result.to_string()]); return; }
        let twin = query("ReadActiveTwinContract", #{});
        if twin == () || twin.name != "Duplicate owner B" { return; }
        let outgoing = record("DuplicateRetiredReuse");
        if outgoing != () {
            // Folder scanning is asynchronous: a copy can install while A
            // remains live. Retained source must still be owned by A, and an
            // uninstalled reservation cannot survive actual A retirement.
            let source = query("GetDocumentSource", #{doc_id: outgoing.doc_id});
            if outgoing.runtime_context != this.expected_owner || source == () || source.source == ()
                || !source.source.contains("fromB") || source.source.contains("successorB") {
                this = finish(this, ["queued A duplicate retained a dead reservation or was retagged into B"]); return;
            }
            this.docs.push(outgoing.doc_id);
            editor::close(outgoing.doc_id);
        } else if this.pending != () {
            let source = query("GetDocumentSource", #{doc_id: this.pending});
            if source != () && source.source != () {
                this.docs.push(this.pending);
                this = finish(this, ["outgoing duplicate installed without its admitted metadata"]); return;
            }
        }
        this.checks.push(t_true(true, "queued original-target work never becomes B-owned; valid pre-close source remains A-owned"));
        let successor = editor::new_scratch(this.source_b, "Duplicate scoped source B");
        if successor.ok != true { this = finish(this, [successor.to_string()]); return; }
        this.b = successor.doc_id; this.docs.push(this.b); this.phase = "source_b"; return;
    }
    if this.phase == "source_b" {
        if !ready(this.b, this.qualified) { return; }
        let entry = document_entry(this.b);
        if entry == () || entry.runtime_context.kind != "local_twin" || entry.runtime_context == this.expected_owner {
            this = finish(this, ["B successor did not receive its distinct exact Twin owner"]); return;
        }
        this.expected_owner = entry.runtime_context;
        this.marker = "successorB"; this.forbidden = "fromB";
        this = request_copy(this, this.qualified, "DuplicateRetiredReuse", "DuplicateRetiredReuse", "DuplicateGateRoot.B.DuplicateRetiredReuse", "done"); return;
    }
    if this.phase == "done" {
        let listed = query("ListOpenDocuments", #{});
        for entry in listed.open_documents {
            if entry.title.starts_with("DuplicateRetiredReuse") && entry.runtime_context != this.expected_owner {
                this = finish(this, ["late outgoing duplicate publication appeared after the valid B successor"]); return;
            }
        }
        let parent_doc = editor::new_scratch(
            "package HeaderImports import Alias = HeaderImports.Shared; type Shared = Real; end HeaderImports;",
            "Duplicate enclosing imports");
        let child = editor::new_scratch(
            "within HeaderImports; model Child /*before*/ \"left\" + /*join*/ \"α right\" output Alias answer(unit=\"K\")=3; end Child;",
            "Duplicate commented description");
        if parent_doc.ok != true || child.ok != true {
            this = finish(this, ["import fixture source admission failed"]); return;
        }
        this.parent_doc = parent_doc.doc_id; this.child = child.doc_id;
        this.docs.push(this.parent_doc); this.docs.push(this.child);
        editor::save_as(this.parent_doc, this.root_b + "/HeaderImports/package.mo");
        editor::save_as(this.child, this.root_b + "/HeaderImports/Child.mo");
        this.phase = "saved_imports"; return;
    }
    if this.phase == "saved_imports" {
        let parent_doc = document_entry(this.parent_doc);
        let child = document_entry(this.child);
        if parent_doc == () || child == () || parent_doc.origin.kind != "file" || child.origin.kind != "file"
            || parent_doc.dirty != false || child.dirty != false || !ready(this.child, "Child") { return; }
        let root_id = "twin:" + this.expected_owner.twin.to_string() + ":Duplicate%20owner%20B:modelica:HeaderImports";
        let loaded = cmd("LoadTwinModelicaSourceRoot", #{twin_id: this.expected_owner.twin,
            name: "Duplicate%20owner%20B", id: root_id, path: "HeaderImports"});
        if loaded == () || loaded.ok != true {
            this = finish(this, ["authored package root admission failed: " + loaded.to_string()]); return;
        }
        this.failure_before = this.errors.len();
        this.copy_ack = cmd("OpenClass", #{qualified: "HeaderImports.Child", action: #{Duplicate: #{name: "HeaderCopy"}}});
        this.phase = "import_copy"; return;
    }
    if this.phase == "import_copy" {
        if this.errors.len() > this.failure_before {
            this = finish(this, ["commented-description duplicate failed: " + this.errors[this.failure_before].to_string()]); return;
        }
        let entry = record("HeaderCopy");
        if entry == () || !ready(entry.doc_id, "HeaderCopy") { return; }
        this.docs.push(entry.doc_id);
        this.import_doc = entry.doc_id;
        let source = query("GetDocumentSource", #{doc_id: entry.doc_id});
        if source == () || source.source == () { this = finish(this, ["commented-description duplicate has no source"]); return; }
        this.checks.push(t_true(source.source.contains("model HeaderCopy /*before*/ \"left\" + /*join*/ \"α right\" output Alias answer")
            && source.source.contains("within HeaderImports;") && entry.runtime_context == this.expected_owner,
            "duplication preserves the parsed description and original lexical scope"));
        this.run_ack = cmd("RunExperiment", #{doc_id: entry.doc_id, class: "HeaderImports.HeaderCopy", t_end: 0.02, n_intervals: 2});
        this.phase = "import_run"; return;
    }
    if this.phase == "import_run" {
        let admitted = ack(this.run_ack);
        if admitted == () || admitted.status == "pending" { return; }
        if admitted.ok != true || admitted.data == () || admitted.data.experiment_id == () {
            this = finish(this, ["lexically scoped duplicate run was rejected: " + admitted.to_string()]); return;
        }
        let run = admitted.data.experiment_id;
        let status = query("RunStatus", #{experiment_id: run});
        if status == () || status.state == "queued" || status.state == "running" { return; }
        if status.state != "done" { this = finish(this, ["lexically scoped duplicate run failed: " + status.to_string()]); return; }
        let result = query("GetExperimentResult", #{experiment_id: run, variables: ["answer"]});
        if result == () || result.series.answer == () || result.metadata.answer == () {
            this = finish(this, ["lexically scoped duplicate omitted its output facts"]); return;
        }
        let correct = result.series.answer.len() == 3 && result.metadata.answer.unit == "K";
        for value in result.series.answer { if value != 3.0 { correct = false; } }
        this.checks.push(t_true(correct, "Rumoca resolves enclosing package imports through the retained within scope"));
        let source = query("GetDocumentSource", #{doc_id: this.import_doc});
        let changed = editor::replace_source(this.import_doc,
            "within HeaderImports; model HeaderCopy output MissingAlias answer=3; end HeaderCopy;", source.generation);
        if changed.ok != true { this = finish(this, ["negative source replacement failed: " + changed.to_string()]); return; }
        this.phase = "invalid_import_parse"; return;
    }
    if this.phase == "invalid_import_parse" {
        let source = query("GetDocumentSource", #{doc_id: this.import_doc});
        if source == () || !source.source.contains("MissingAlias") || !ready(this.import_doc, "HeaderCopy") { return; }
        this.run_ack = cmd("RunExperiment", #{doc_id: this.import_doc, class: "HeaderImports.HeaderCopy", t_end: 0.02, n_intervals: 2});
        this.phase = "invalid_import_run"; return;
    }
    if this.phase == "invalid_import_run" {
        let admitted = ack(this.run_ack);
        if admitted == () || admitted.status == "pending" { return; }
        if admitted.ok != true || admitted.data == () || admitted.data.experiment_id == () {
            this = finish(this, ["negative run admission failed: " + admitted.to_string()]); return;
        }
        let status = query("RunStatus", #{experiment_id: admitted.data.experiment_id});
        if status == () || status.state == "queued" || status.state == "running" { return; }
        this.checks.push(t_true(status.state == "failed" && status.has_result == false
            && status.error != () && status.error.contains("unresolved type reference")
            && status.error.contains("MissingAlias"),
            "Rumoca rejects missing package types without reusing the successful compiled result"));
        this = finish(this, this.checks);
    }
}
