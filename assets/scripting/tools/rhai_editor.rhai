// Source-backed Rhai script editor facade.
//
// The ScriptRegistry/DocumentHost owns source, UTF-8 validation, generation,
// journaling and history. This module only provides the dynamic Editor
// vocabulary; it never evaluates or writes a script outside the document
// command boundary.

fn _error(stage, message) {
    #{ ok: false, domain: "rhai", stage: stage, retryable: false,
       error: message, errors: [message] }
}

fn _doc_required(doc, operation) {
    if doc == () || doc.to_string() == "0" {
        return _error(operation, "an explicit Rhai document id is required");
    }
    #{ ok: true }
}

fn inspect(doc) {
    let valid = _doc_required(doc, "inspect");
    if !valid.ok { return valid; }
    query("InspectScriptDocument", #{ doc_id: doc })
}

fn apply(doc, ops, parent_generation) {
    let valid = _doc_required(doc, "apply");
    if !valid.ok { return valid; }
    if ops == () || type_of(ops) != "array" || ops.is_empty() {
        return _error("apply", "a non-empty Rhai operation array is required");
    }
    cmd("ApplyScriptOps", #{
        doc_id: doc,
        ops: ops,
        parent_generation: parent_generation,
    })
}

fn replace_source(doc, source, parent_generation) {
    apply(doc, [#{ ReplaceSource: #{ source: source } }], parent_generation)
}

fn edit_text(doc, range_start, range_end, replacement, parent_generation) {
    if range_start == () || range_end == () || replacement == ()
        || type_of(replacement) != "string" {
        return _error("edit_text", "start, end, and replacement are required");
    }
    if range_start < 0 || range_end < range_start {
        return _error("edit_text", "range must be ordered and non-negative");
    }
    apply(doc, [#{ EditText: #{
        range_start: range_start,
        range_end: range_end,
        replacement: replacement,
    } }], parent_generation)
}

fn verify(doc) {
    let inspection = inspect(doc);
    if inspection == () || inspection.ok != true {
        return _error("inspect", "InspectScriptDocument failed");
    }
    let status = inspection.status;
    if status == () || status.ok != true {
        return #{ ok: false, domain: "rhai", stage: "compile",
            retryable: status != () && status.state == "compiling",
            inspection: inspection,
            error: "Rhai source is not compiled cleanly" };
    }
    #{ ok: true, domain: "rhai", stage: "verified", retryable: false,
       inspection: inspection, generation: inspection.generation }
}

fn undo(doc) { cmd("UndoDocument", #{ doc_id: doc }) }
fn redo(doc) { cmd("RedoDocument", #{ doc_id: doc }) }
