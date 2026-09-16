// Source-backed Modelica editor facade.
//
// AST/diagram lowering, parse scheduling, journaling and undo are Rust-owned;
// this facade keeps document identity, generation cursors and checkpoint UX
// consistent with USD and SysML.

fn _error(stage, message) {
    #{ ok: false, domain: "modelica", stage: stage, retryable: false,
       error: message, errors: [message] }
}

fn _doc_required(doc, operation) {
    if doc == () || doc == 0 {
        return _error(operation, "an explicit Modelica document id is required");
    }
    #{ ok: true }
}

fn open(path) { cmd("OpenFile", #{ path: path }) }
fn new_document() { cmd("NewDocument", #{ kind: "modelica" }) }

fn inspect(doc) {
    let valid = _doc_required(doc, "inspect");
    if !valid.ok { return valid; }
    let source = query("GetDocumentSource", #{ doc_id: doc });
    if source == () || source.ok != true { return source; }
    let compile = query("CompileStatus", #{ doc_id: doc });
    if compile == () || compile.ok != true {
        return _error("compile_status", "CompileStatus failed");
    }
    #{ ok: true, domain: "modelica", source: source, compile: compile,
       generation: source.generation, dirty: source.dirty }
}

/// Apply the existing prelude ApiOp constructors as one generation-checked
/// command. The Modelica owner reparses/defer-schedules structural operations
/// without blocking the Editor thread.
fn apply(doc, ops, parent_generation) {
    let valid = _doc_required(doc, "apply");
    if !valid.ok { return valid; }
    if ops == () || type_of(ops) != "array" || ops.is_empty() {
        return _error("apply", "a non-empty Modelica operation array is required");
    }
    modelica_apply_against(doc, ops, parent_generation)
}

/// Source replacement is explicit and remains undoable. Prefer structured
/// ApiOps for component/connection/diagram edits.
fn replace_source(doc, source, parent_generation) {
    apply(doc, [modelica_replace_source(source)], parent_generation)
}

fn verify(doc) {
    let state = inspect(doc);
    if state == () || state.ok != true { return state; }
    let compile = state.compile;
    let clean = compile.state == "ok" && compile.ast_parsed == true
        && compile.error_message == ();
    if !clean {
        return #{ ok: false, domain: "modelica", stage: "compile",
            retryable: compile.state == "compiling", inspection: state,
            error: "Modelica source is not compiled cleanly" };
    }
    #{ ok: true, domain: "modelica", stage: "verified", retryable: false,
       inspection: state, generation: state.generation }
}

fn save(doc) { cmd("SaveDocument", #{ doc_id: doc }) }
fn save_as(doc, path) { cmd("SaveAsDocument", #{ doc_id: doc, path: path }) }
fn undo(doc) { cmd("UndoDocument", #{ doc_id: doc }) }
fn redo(doc) { cmd("RedoDocument", #{ doc_id: doc }) }
