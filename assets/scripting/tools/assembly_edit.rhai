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

/// Create a new untitled USD assembly document.
fn new_document() {
    cmd("NewDocument", #{ kind: "usd" })
}

/// Fork one explicit USD document into an independently editable assembly.
/// The result is asynchronous; discover the new DocumentId with
/// `ListOpenDocuments`.
fn fork_document(source, name) {
    cmd("ForkDocument", #{ source: source, name: name })
}

/// Persist one explicit USD document at its current file identity.
fn save_document(doc) {
    cmd("SaveDocument", #{ doc: doc })
}

/// Persist one explicit USD document at a new path. An empty path uses the
/// normal Save-As picker owned by the workbench.
fn save_as_document(doc, path) {
    cmd("SaveAsDocument", #{ doc: doc, path: path })
}

/// Close one explicit USD document and release its projection/session state.
fn close_document(doc) {
    cmd("CloseDocument", #{ doc: doc })
}

/// Discard one explicit USD document's local edits through its owner.
fn discard_document(doc) {
    cmd("DiscardDocument", #{ doc: doc })
}

/// Open one explicit isolated Editor preview. `preview` identifies the
/// session, so another document can remain rendered independently.
fn preview_open(preview, doc, edit_target) {
    cmd("OpenUsdPreview", #{
        preview: preview,
        doc: doc,
        edit_target: edit_target,
    })
}

/// Open one additional dockable view over an existing preview session. `view`
/// is an explicit stable presentation identity; all views of the session share
/// the same projected USD stage.
fn preview_view_open(preview, view) {
    cmd("OpenUsdPreviewView", #{
        preview: preview,
        view: view,
    })
}

/// Focus one exact presentation view.
fn preview_view_focus(view) {
    cmd("FocusUsdPreviewView", #{ view: view })
}

/// Close one presentation view. Closing the final view closes its parent
/// preview session.
fn preview_view_close(view) {
    cmd("CloseUsdPreviewView", #{ view: view })
}

/// Focus an existing Editor preview session in the desktop USD dock.
fn preview_focus(preview) {
    cmd("FocusUsdPreview", #{ preview: preview })
}

/// Close one Editor preview session and release its presentation resources.
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

/// Validate and queue an explicit proposal without mutating the document.
/// `scope` is one of `SourceAsset`, `Assembly`, or `InstanceOverride`; the
/// proposal must carry the generation the caller inspected.
fn propose(doc, scope, label, ops, parent_gen) {
    cmd("CreateUsdProposal", #{
        doc: doc,
        scope: scope,
        label: label,
        parent_gen: parent_gen,
        ops: ops,
    })
}

/// Inspect pending proposals, their typed operations, diagnostics, and stale
/// state for one explicit document.
fn review_session(doc) {
    query("InspectUsdEditSession", #{ doc: doc })
}

/// Mute, unmute, or reject one proposal. Use `#{ Mute: () }`,
/// `#{ Unmute: () }`, or `#{ Reject: () }` for `action`.
fn review_proposal(proposal, action) {
    cmd("ReviewUsdProposal", #{ proposal: proposal, action: action })
}

/// Commit one still-pending proposal. The USD owner rechecks the generation,
/// source identity, file watermark, and typed operation validation before it
/// enters the ordinary journal/undo change-set path.
fn commit_proposal(proposal) {
    cmd("CommitUsdProposal", #{ proposal: proposal })
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
