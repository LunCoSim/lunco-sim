// Explicit USD assembly editing for agents and editor automation.
//
// This library is policy only. The document registry, OpenUSD composition,
// validation, journaling, undo/redo, and live projection remain behind the
// existing typed command/query boundary. `doc` is always supplied by the
// caller; `edit_target` is always supplied for authored operations.
//
// `parent_gen` is a document generation cursor. Pass `()` when the caller has
// no causal predecessor; pass the generation returned by Inspect/Sync or a
// previous command acknowledgement to reject stale edits atomically.

/// Open a USD source through the normal asynchronous document lifecycle.
///
/// The command response acknowledges admission, not a fabricated document id.
/// Discover the resulting explicit id with `ListOpenDocuments`.
fn open(path) {
    cmd("OpenFile", #{ path: path })
}

/// Open one explicit isolated Editor preview. `preview` identifies the
/// session lease, so another document can remain rendered independently.
fn preview_open(preview, doc, edit_target) {
    cmd("OpenUsdPreview", #{
        preview: preview,
        doc: doc,
        edit_target: edit_target,
    })
}

/// Focus an existing Editor preview lease in the desktop USD dock.
fn preview_focus(preview) {
    cmd("FocusUsdPreview", #{ preview: preview })
}

/// Close one Editor preview lease and release its presentation resources.
fn preview_close(preview) {
    cmd("CloseUsdPreview", #{ preview: preview })
}

/// Describe one explicit USD document, including layers, revisions,
/// composition dependencies, diagnostics, and journal position.
fn describe(doc) {
    query("InspectUsdDocument", #{ doc: doc })
}

/// Inspect one composed local prim in an explicit document.
fn inspect(doc, path) {
    query("InspectUsdDocument", #{ doc: doc, path: path })
}

/// Resolve one explicit composed prim and authored edit layer.
fn resolve_target(doc, path, edit_target) {
    query("ResolveUsdTarget", #{
        doc: doc,
        path: path,
        edit_target: edit_target,
    })
}

/// Synchronize from a document generation. `since_generation == ()` requests
/// the complete root/runtime snapshot; a covered cursor returns typed ops.
fn sync_document(doc, since_generation) {
    if since_generation == () {
        query("SyncUsdDocument", #{ doc: doc })
    } else {
        query("SyncUsdDocument", #{
            doc: doc,
            since_generation: since_generation,
        })
    }
}

/// Add a prim or reference through the existing typed USD authoring op.
fn add_prim(doc, edit_target, parent_path, name, type_name, reference, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ AddPrim: #{
            edit_target: edit_target,
            parent_path: parent_path,
            name: name,
            type_name: type_name,
            reference: reference,
        }},
    })
}

/// Apply translation and/or Euler XYZ rotation as one journaled edit.
/// `translation` and `rotation` may be `()` independently, but not both.
fn transform(doc, edit_target, path, translation, rotation, parent_gen) {
    let ops = [];
    if translation != () {
        ops.push(#{ SetTranslate: #{
            edit_target: edit_target,
            path: path,
            value: translation,
        }});
    }
    if rotation != () {
        ops.push(#{ SetRotate: #{
            edit_target: edit_target,
            path: path,
            value: rotation,
        }});
    }
    if ops.is_empty() {
        return #{ ok: false, error: "transform requires translation or rotation" };
    }
    cmd("ApplyUsdOps", #{
        doc: doc,
        parent_gen: parent_gen,
        label: "Editor transform",
        ops: ops,
    })
}

/// Set one typed USD attribute in an explicit authored layer.
fn attribute(doc, edit_target, path, name, type_name, value, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetAttribute: #{
            edit_target: edit_target,
            path: path,
            name: name,
            type_name: type_name,
            value: value,
        }},
    })
}

/// Author one USD time sample through the existing reversible keyframe op.
/// `time` is a USD time code and `value` is a literal in `type_name`'s USD
/// syntax. For xform channels, the document owner also maintains
/// `xformOpOrder` when the first channel is keyed.
fn keyframe(doc, edit_target, path, name, type_name, time, value, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetTimeSample: #{
            edit_target: edit_target,
            path: path,
            name: name,
            type_name: type_name,
            time: time,
            value: value,
        }},
    })
}

/// Remove one exact USD time sample through the existing reversible keyframe op.
fn remove_keyframe(doc, edit_target, path, name, time, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ RemoveTimeSample: #{
            edit_target: edit_target,
            path: path,
            name: name,
            time: time,
        }},
    })
}

/// Set one USD relationship in an explicit authored layer.
fn relationship(doc, edit_target, path, name, targets, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetRelationship: #{
            edit_target: edit_target,
            path: path,
            name: name,
            targets: targets,
        }},
    })
}

/// Set one USD connection in an explicit authored layer.
fn connection(doc, edit_target, path, name, type_name, sources, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetConnection: #{
            edit_target: edit_target,
            path: path,
            name: name,
            type_name: type_name,
            sources: sources,
        }},
    })
}

/// Apply API schemas through the existing typed USD authoring op.
fn schema(doc, edit_target, path, schemas, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetApiSchemas: #{
            edit_target: edit_target,
            path: path,
            schemas: schemas,
        }},
    })
}

/// Select a variant in an explicit authored layer.
fn variant(doc, edit_target, path, variant_set, selection, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc: doc,
        parent_gen: parent_gen,
        op: #{ SetVariantSelection: #{
            edit_target: edit_target,
            path: path,
            variant_set: variant_set,
            variant: selection,
        }},
    })
}

/// Submit a caller-built typed USD plan as one journal and undo unit.
/// Every op must contain its own explicit `edit_target` as required by `UsdOp`.
fn batch(doc, label, ops, parent_gen) {
    cmd("ApplyUsdOps", #{
        doc: doc,
        parent_gen: parent_gen,
        label: label,
        ops: ops,
    })
}

/// Attach a component through the existing mount/socket/joint validator and
/// lowering path. `spec` is the reflected `AttachSpec` contract.
fn attach_component(doc, spec) {
    cmd("AttachComponent", #{ doc: doc, spec: spec })
}

/// Detach a component through the existing topology validator and change-set
/// path. The spec must identify the exact component, joint, and socket paths.
fn detach_component(doc, spec) {
    cmd("DetachComponent", #{ doc: doc, spec: spec })
}

/// Undo the latest history group on one explicit document.
fn undo(doc) {
    cmd("UndoDocument", #{ doc: doc })
}

/// Redo the latest undone history group on one explicit document.
fn redo(doc) {
    cmd("RedoDocument", #{ doc: doc })
}
