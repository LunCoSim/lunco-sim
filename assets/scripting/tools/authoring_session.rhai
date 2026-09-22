// Cross-domain interactive authoring session.
//
// This is policy/UX only. It does not create a shadow graph or bypass a
// document owner. Each transaction is explicit, dry-plannable, grouped into
// one journal/undo entry, generation-checked, and followed by a checkpoint.

import "lunco://scripting/tools/assembly_edit" as usd;
import "lunco://scripting/tools/modelica_editor" as modelica;
import "lunco://scripting/tools/sysml_editor" as sysml;
import "lunco://scripting/tools/rhai_editor" as rhai;
import "lunco://scripting/tools/editor_workflow" as usd_workflow;

fn _error(stage, message) {
    #{ ok: false, kind: "authoring_session", stage: stage,
       retryable: false, error: message, errors: [message] }
}

fn _domain(domain) {
    if domain == "usd" || domain == "modelica" || domain == "sysml" || domain == "rhai" {
        return #{ ok: true, domain: domain };
    }
    _error("domain", "unsupported authoring domain: " + domain)
}

/// Capability discovery used by Editor menus and agent planning.
fn capabilities(domain) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if domain == "usd" {
        return #{ ok: true, domain: domain, dry_plan: true,
            grouped_apply: true, generation_checked: true, undoable: true,
            preview: true, selection: true, projection_checkpoint: true,
            lifecycle: ["open", "new", "save", "save_as"],
            operations: ["typed_usd_ops", "proposal_review", "batch"] };
    }
    if domain == "modelica" {
        return #{ ok: true, domain: domain, dry_plan: true,
            grouped_apply: true, generation_checked: true, undoable: true,
            preview: false, source_checkpoint: true,
            lifecycle: ["open", "new", "save", "save_as"],
            operations: ["ast_ops", "diagram_ops", "text_ops"] };
    }
    if domain == "rhai" {
        return #{ ok: true, domain: domain, dry_plan: true,
            grouped_apply: true, generation_checked: true, undoable: true,
            preview: false, source_checkpoint: true,
            // Rhai source is owned by the Twin and loaded through its indexed
            // source boundary. It has no standalone file-document lifecycle;
            // advertising open/new/save here would make the Editor offer
            // commands that the owner intentionally rejects.
            lifecycle: ["inspect", "edit", "undo", "redo"],
            twin_authored_source: true,
            operations: ["replace_source", "edit_text"] };
    }
    #{ ok: true, domain: domain, dry_plan: true,
        grouped_apply: true, generation_checked: true, undoable: true,
        preview: false, source_checkpoint: true,
        lifecycle: ["open", "new", "save", "save_as"],
        twin_authored_source: false,
        operations: ["replace_source", "edit_text"] }
}

/// Open a source document through the owner-specific asynchronous lifecycle.
/// The command acknowledgement is admission only; callers discover the
/// resulting explicit document id through the normal document query.
fn open(domain, path) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if path == () || type_of(path) != "string" || path == "" {
        return _error("open", "a non-empty source path is required");
    }
    if domain == "usd" { return usd::open(path); }
    if domain == "modelica" { return modelica::open(path); }
    if domain == "rhai" { return _error("open", "Rhai documents are Twin-authored source"); }
    sysml::open(path)
}

/// Create an untitled document owned by the selected format adapter.
fn new_document(domain) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if domain == "usd" { return usd::new_document(); }
    if domain == "modelica" { return modelica::new_document(); }
    if domain == "rhai" { return _error("new", "Rhai documents are Twin-authored source"); }
    sysml::new_document()
}

/// Persist one explicit document through its format owner.
fn save(domain, doc) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    let required = inspect(domain, doc);
    if !required.ok { return required; }
    if domain == "usd" { return usd::save_document(doc); }
    if domain == "modelica" { return modelica::save(doc); }
    if domain == "rhai" { return _error("save", "Rhai source is Twin-authored; use Twin persistence"); }
    sysml::save(doc)
}

/// Persist one explicit document to a new path through its format owner.
fn save_as(domain, doc, path) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if path == () || type_of(path) != "string" || path == "" {
        return _error("save_as", "a non-empty destination path is required");
    }
    let required = inspect(domain, doc);
    if !required.ok { return required; }
    if domain == "usd" { return usd::save_as_document(doc, path); }
    if domain == "modelica" { return modelica::save_as(doc, path); }
    if domain == "rhai" { return _error("save_as", "Rhai source is Twin-authored; use Twin persistence"); }
    sysml::save_as(doc, path)
}

/// Read the owner-specific identity/generation snapshot.
fn inspect(domain, doc) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if doc == () || doc == 0 {
        return _error("inspect", "an explicit document id is required");
    }
    if domain == "usd" { return usd::describe(doc); }
    if domain == "modelica" { return modelica::inspect(doc); }
    if domain == "rhai" { return rhai::inspect(doc); }
    sysml::inspect(doc)
}

fn _operation_names(domain, ops) {
    let names = [];
    for op in ops {
        if domain == "sysml" {
            if type_of(op) != "map" || op.keys().len() != 1 {
                return _error("plan", "each SysML operation must have one typed variant");
            }
            names.push(op.keys()[0]);
        } else if domain == "usd" {
            if type_of(op) != "map" || op.keys().len() != 1 {
                return _error("plan", "each USD operation must have one typed variant");
            }
            let body = op[op.keys()[0]];
            if body == () || body.edit_target == () || body.edit_target == "" {
                return _error("plan", "every USD operation needs an explicit edit_target");
            }
            names.push(op.keys()[0]);
        } else if domain == "modelica" {
            if type_of(op) != "map" && type_of(op) != "string" {
                return _error("plan", "each Modelica operation must be a typed ApiOp");
            }
            if type_of(op) == "map" && op.keys().len() != 1 {
                return _error("plan", "each Modelica operation must have one typed variant");
            }
            if type_of(op) == "string" {
                names.push(op);
            } else {
                names.push(op.keys()[0]);
            }
        } else {
            if type_of(op) != "map" || op.keys().len() != 1 {
                return _error("plan", "each Rhai operation must have one typed variant");
            }
            names.push(op.keys()[0]);
        }
    }
    names.sort();
    names
}

/// Pure, deterministic operation review. It never calls `cmd` or mutates a
/// document, so Editor preview/approval can call it repeatedly.
fn plan(domain, ops, label, parent_generation) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if ops == () || type_of(ops) != "array" || ops.is_empty() {
        return _error("plan", "a non-empty operation array is required");
    }
    if label == () || type_of(label) != "string" || label == "" {
        return _error("plan", "a non-empty transaction label is required");
    }
    let names = _operation_names(domain, ops);
    if type_of(names) == "map" && names.ok == false { return names; }
    #{ ok: true, kind: "authoring_session", mode: "dry",
       domain: domain, label: label, operation_count: ops.len(),
       operation_kinds: names, parent_generation: parent_generation,
       undo_group: label, mutation: "not_submitted" }
}

/// Apply one reviewed plan. Callers must pass the generation they inspected;
/// stale edits are rejected by the domain owner, never retargeted implicitly.
fn apply(domain, doc, ops, label, parent_generation) {
    let reviewed = plan(domain, ops, label, parent_generation);
    if !reviewed.ok { return reviewed; }
    let result = if domain == "usd" {
        usd::batch(doc, label, ops, parent_generation)
    } else if domain == "modelica" {
        modelica::apply(doc, ops, parent_generation)
    } else if domain == "rhai" {
        rhai::apply(doc, ops, parent_generation)
    } else {
        sysml::apply(doc, ops, parent_generation)
    };
    if result == () || result.ok == false {
        return #{ ok: false, kind: "authoring_session", stage: "apply",
            retryable: false, plan: reviewed, result: result,
            error: "typed authoring command was not admitted" };
    }
    #{ ok: true, kind: "authoring_session", stage: "applied",
       plan: reviewed, result: result, checkpoint_required: true }
}

/// Inspect/lint/compile/project after a grouped apply. USD uses the existing
/// projection-aware workflow; source domains use their owner checkpoints.
fn checkpoint(domain, doc) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if domain == "usd" { return usd_workflow::after_edit(doc); }
    if domain == "modelica" { return modelica::verify(doc); }
    if domain == "rhai" { return rhai::verify(doc); }
    sysml::verify(doc)
}

fn undo(domain, doc) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if domain == "usd" { return usd::undo(doc); }
    if domain == "modelica" { return modelica::undo(doc); }
    if domain == "rhai" { return rhai::undo(doc); }
    sysml::undo(doc)
}

fn redo(domain, doc) {
    let valid = _domain(domain);
    if !valid.ok { return valid; }
    if domain == "usd" { return usd::redo(doc); }
    if domain == "modelica" { return modelica::redo(doc); }
    if domain == "rhai" { return rhai::redo(doc); }
    sysml::redo(doc)
}
