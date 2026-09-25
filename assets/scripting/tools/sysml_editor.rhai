// Source-backed SysML v2 editor facade.
//
// Rust owns the document registry, UTF-8/range validation, semantic refresh,
// journaling and history. This library owns the interactive vocabulary and
// deliberately requires an explicit document and inspected generation.

fn _error(stage, message) {
    #{ ok: false, domain: "sysml", stage: stage, retryable: false,
       error: message, errors: [message] }
}

fn _doc_required(doc, operation) {
    if doc == () || doc.to_string() == "0" {
        return _error(operation, "an explicit SysML document id is required");
    }
    #{ ok: true }
}

/// Open a SysML/KerML source through the normal document lifecycle.
fn open(path) { cmd("OpenFile", #{ path: path }) }

/// Create an editable untitled SysML source document.
fn new_document() { cmd("NewDocument", #{ kind: "sysml" }) }

/// Persist one explicit SysML document.
fn save(doc) {
    let valid = _doc_required(doc, "save");
    if !valid.ok { return valid; }
    cmd("SaveSysmlDocument", #{ doc_id: doc })
}

/// Persist one explicit SysML document to a new file.
fn save_as(doc, path) {
    let valid = _doc_required(doc, "save_as");
    if !valid.ok { return valid; }
    if path == () || type_of(path) != "string" || path == "" {
        return _error("save_as", "a non-empty destination path is required");
    }
    cmd("SaveAsDocument", #{ doc_id: doc, path: path })
}

/// Read source, generation, origin and parser/resolver diagnostics for one
/// explicit document. Semantic requirement records come from `AnalyzeSysml`.
fn inspect(doc) {
    let valid = _doc_required(doc, "inspect");
    if !valid.ok { return valid; }
    query("InspectSysmlDocument", #{ doc_id: doc })
}

/// Replace the complete source as one undoable edit. Prefer `edit_text` for a
/// small change so unrelated source formatting and comments stay untouched.
fn replace_source(doc, source, parent_generation) {
    let valid = _doc_required(doc, "replace_source");
    if !valid.ok { return valid; }
    if source == () || type_of(source) != "string" {
        return _error("replace_source", "source must be UTF-8 text");
    }
    cmd("ApplySysmlOps", #{
        doc_id: doc,
        parent_generation: parent_generation,
        ops: [#{ ReplaceSource: #{ source: source } }],
    })
}

/// Replace one UTF-8 byte range. Offsets are lossless integers and are checked
/// by the Rust document owner for bounds and character boundaries.
fn edit_text(doc, range_start, range_end, replacement, parent_generation) {
    let valid = _doc_required(doc, "edit_text");
    if !valid.ok { return valid; }
    if range_start == () || range_end == () || replacement == ()
        || type_of(replacement) != "string" {
        return _error("edit_text", "start, end, and replacement are required");
    }
    if range_start < 0 || range_end < range_start {
        return _error("edit_text", "range must be ordered and non-negative");
    }
    cmd("ApplySysmlOps", #{
        doc_id: doc,
        parent_generation: parent_generation,
        ops: [#{ EditText: #{
            range_start: range_start,
            range_end: range_end,
            replacement: replacement,
        } }],
    })
}

/// Submit already reviewed source operations as one undo/journal group.
fn apply(doc, ops, parent_generation) {
    let valid = _doc_required(doc, "apply");
    if !valid.ok { return valid; }
    if ops == () || type_of(ops) != "array" || ops.is_empty() {
        return _error("apply", "a non-empty SysML operation array is required");
    }
    cmd("ApplySysmlOps", #{
        doc_id: doc,
        parent_generation: parent_generation,
        ops: ops,
    })
}

/// Check the current source snapshot before an apply or save checkpoint.
/// Analysis state is revision-scoped and may be pending while the shared
/// worker prepares parser/resolver facts. File-backed documents additionally
/// expose the full Twin source-set validation report.
fn verify(doc) {
    let inspection = inspect(doc);
    // Successful read-only queries may omit `ok`; only an explicit false
    // indicates rejection. Requiring true made the editor report valid,
    // generation-bearing InspectSysmlDocument responses as failures.
    if inspection == () || inspection.ok == false {
        return _error("inspect", "InspectSysmlDocument failed");
    }
    if inspection.analysis_state == "pending" {
        return #{ ok: false, domain: "sysml", stage: "analysis",
            retryable: true, inspection: inspection,
            error: "SysML analysis is still preparing for this source revision" };
    }
    if inspection.analysis_state == "failed" {
        return #{ ok: false, domain: "sysml", stage: "analysis",
            retryable: false, inspection: inspection,
            error: inspection.analysis_error };
    }
    if inspection.analysis_state != "ready" {
        return _error("analysis", "SysML analysis returned an unknown state");
    }
    if inspection.semantic_errors == true {
        return #{ ok: false, domain: "sysml", stage: "semantic",
            retryable: false, inspection: inspection,
            error: "SysML source has parser or resolver diagnostics" };
    }
    let uri = inspection.origin.uri;
    let report = ();
    if uri.starts_with("twin://") || uri.ends_with(".sysml") || uri.ends_with(".kerml") {
        report = sysml_report(uri);
    }
    if report != () && report.ok == false {
        return #{ ok: false, domain: "sysml", stage: "validation",
            retryable: false, inspection: inspection, report: report,
            error: "AnalyzeSysml rejected the source" };
    }
    #{ ok: true, domain: "sysml", stage: "verified", retryable: false,
       inspection: inspection, report: report,
       generation: inspection.generation }
}

/// Undo/redo are generic document verbs; the SysML owner dispatches them to
/// its own history stack. They never target an active tab implicitly.
fn undo(doc) { cmd("UndoDocument", #{ doc_id: doc }) }
fn redo(doc) { cmd("RedoDocument", #{ doc_id: doc }) }
