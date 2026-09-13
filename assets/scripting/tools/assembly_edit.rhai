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
    cmd("ForkDocument", #{ source_doc_id: source, name: name })
}

/// Persist one explicit USD document at its current file identity.
fn save_document(doc) {
    cmd("SaveDocument", #{ doc_id: doc })
}

/// Persist one explicit USD document at a new path. An empty path uses the
/// normal Save-As picker owned by the workbench.
fn save_as_document(doc, path) {
    cmd("SaveAsDocument", #{ doc_id: doc, path: path })
}

/// Close one explicit USD document and release its projection/session state.
fn close_document(doc) {
    cmd("CloseDocument", #{ doc_id: doc })
}

/// Discard one explicit USD document's local edits through its owner.
fn discard_document(doc) {
    cmd("DiscardDocument", #{ doc_id: doc })
}

/// Open one explicit isolated Editor preview. `preview` identifies the
/// session, so another document can remain rendered independently.
fn preview_open(preview, doc, edit_target) {
    cmd("OpenUsdPreview", #{
        preview: preview,
        doc_id: doc,
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

/// Set the presentation projection of one explicit preview view. This does
/// not rewrite an authored USD camera opinion.
fn preview_view_projection(view, projection) {
    cmd("SetUsdPreviewProjection", #{
        view: view,
        projection: projection,
    })
}

/// Switch one preview view between the shared Visual and Text presentations.
/// Both modes retain the same document, projected stage, selection, and view
/// camera; Text is a read-only inspection surface.
fn preview_view_mode(view, mode) {
    cmd("SetUsdPreviewViewMode", #{
        view: view,
        mode: mode,
    })
}

/// Choose the authored or composed USDA snapshot shown by Text mode.
fn preview_view_text_layer(view, layer) {
    cmd("SetUsdPreviewTextLayer", #{
        view: view,
        layer: layer,
    })
}

/// Fit one preview view to the projected visual bounds.
fn preview_view_frame(view) {
    cmd("FrameUsdPreviewView", #{ view: view })
}

/// Fit one view to the visual bounds of an exact composed prim subtree. The
/// preview lease scopes the path; the live scene is never selected by name.
fn preview_view_frame_path(preview, view, path) {
    cmd("FrameUsdPreviewSelection", #{ preview: preview, view: view, path: path })
}

/// Restore one preview view's default orbit pose and refit it.
fn preview_view_reset(view) {
    cmd("ResetUsdPreviewView", #{ view: view })
}

/// Pan one preview view by screen-pixel units.
fn preview_view_pan(view, delta) {
    cmd("PanUsdPreviewView", #{ view: view, delta: delta })
}

/// Zoom one preview view by a positive multiplicative factor.
fn preview_view_zoom(view, factor) {
    cmd("ZoomUsdPreviewView", #{ view: view, factor: factor })
}

/// Persist the current view-only camera pose in the shared settings document.
fn save_inspection_preset(view, name) {
    cmd("SaveUsdInspectionPreset", #{ view: view, name: name })
}

fn apply_inspection_preset(view, name) {
    cmd("ApplyUsdInspectionPreset", #{ view: view, name: name })
}

fn delete_inspection_preset(name) {
    cmd("DeleteUsdInspectionPreset", #{ name: name })
}

fn inspection_presets() {
    query("InspectUsdInspectionPresets", #{})
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

/// Apply a transient CAD-style explode pose to explicit projected USD parts.
/// `assembly` and every entry in `parts` are absolute composed prim paths.
/// The operation is session-scoped and never authors USD.
fn preview_explode(preview, doc, assembly, parts, action, axis, spacing) {
    let params = #{
        preview: preview,
        doc_id: doc,
        assembly: assembly,
        parts: parts,
        action: action,
        // Reflection uses the typed enum variant names on the command bus;
        // explicit nulls keep reset constructible without a fake spacing.
        axis: axis,
        spacing: spacing,
    };
    cmd("ExplodeUsdPreview", params)
}

/// Enable an explode pose. Parts are ordered by canonical USD path before
/// offsets are computed, making repeated calls deterministic.
fn preview_explode_enable(preview, doc, assembly, parts, axis, spacing) {
    preview_explode(preview, doc, assembly, parts, "Enable", axis, spacing)
}

/// Update the axis/spacing of an existing explode pose without recapturing
/// the original local transforms.
fn preview_explode_update(preview, doc, assembly, parts, axis, spacing) {
    preview_explode(preview, doc, assembly, parts, "Update", axis, spacing)
}

/// Reset the explicit explode target to its captured, unmodified preview pose.
fn preview_explode_reset(preview, doc, assembly, parts) {
    preview_explode(preview, doc, assembly, parts, "Reset", (), ())
}

/// Describe one explicit USD document, including layers, revisions,
/// composition dependencies, diagnostics, and journal position.
fn describe(doc) {
    query("InspectUsdDocument", #{ doc_id: doc })
}

/// Inspect one composed local prim in an explicit document.
fn inspect(doc, path) {
    query("InspectUsdDocument", #{ doc_id: doc, path: path })
}

/// Inspect the exact USD previews and presentation views currently open in
/// the Editor. This is the source of truth for correlating a screenshot with
/// its explicit document and view handles.
fn viewport() {
    query("InspectUsdViewport", #{})
}

/// Read the focused Editor preview's canonical selection context. The result
/// contains document/preview/path identity, composed USD type and kind,
/// parent/assembly paths, supported typed operations, primary selection, and
/// any explicit stale or ambiguous state.
fn selection_context() {
    query("InspectUsdSelection", #{})
}

/// Read one explicit open Editor preview's canonical selection context. Use
/// this for a hidden view without changing which preview the user sees.
fn selection_context_for(preview) {
    query("InspectUsdSelection", #{ preview: preview })
}

/// Resolve one explicit composed prim and authored edit layer.
fn resolve_target(doc, path, edit_target) {
    query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: edit_target,
    })
}

/// Synchronize from a document generation. `since_generation == ()` requests
/// the complete root/runtime snapshot; a covered cursor returns typed ops.
fn sync_document(doc, since_generation) {
    if since_generation == () {
        query("SyncUsdDocument", #{ doc_id: doc })
    } else {
        query("SyncUsdDocument", #{
            doc_id: doc,
            since_generation: since_generation,
        })
    }
}

/// Declare a connected scalar program input for `attach_program`.
fn program_input_connection(name, source) {
    #{
        name: name,
        type_name: "float",
        default_value: (),
        connection: source,
    }
}

/// Declare a constant scalar program input for `attach_program`.
fn program_input_default(name, value) {
    #{
        name: name,
        type_name: "float",
        default_value: value,
        connection: (),
    }
}

/// Declare a scalar program output and its consuming USD input properties.
fn program_output(name, connections) {
    #{
        name: name,
        type_name: "float",
        connections: connections,
    }
}

/// Add a prim or reference through the existing typed USD authoring op.
fn add_prim(doc, edit_target, parent_path, name, type_name, reference, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ AddPrim: #{
            edit_target: edit_target,
            parent_path: parent_path,
            name: name,
            type_name: type_name,
            reference: reference,
            reference_prim_path: "",
        }},
    })
}

/// Remove one exact prim subtree through the existing typed USD authoring op.
/// The caller chooses the authored layer and supplies the inspected path.
fn remove_prim(doc, edit_target, path, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ RemovePrim: #{
            edit_target: edit_target,
            path: path,
        }},
    })
}

/// Rename or reparent one exact prim through the reversible namespace op.
/// The destination parent must already exist in the authored USD layer.
fn move_prim(doc, edit_target, from_path, to_path, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ MovePrim: #{
            edit_target: edit_target,
            from_path: from_path,
            to_path: to_path,
        }},
    })
}

/// Apply translation, Euler XYZ rotation, and/or unitless local scale as one
/// journaled edit. The transform components may be `()` independently, but
/// not all three.
fn transform(doc, edit_target, path, translation, rotation, scale, parent_gen) {
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
    if scale != () {
        ops.push(#{ SetScale: #{
            edit_target: edit_target,
            path: path,
            value: scale,
        }});
    }
    if ops.is_empty() {
        return #{ ok: false, error: "transform requires translation, rotation, or scale" };
    }
    cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: parent_gen,
        label: "Editor transform",
        ops: ops,
    })
}

/// Set one typed USD attribute in an explicit authored layer.
fn attribute(doc, edit_target, path, name, type_name, value, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
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
        doc_id: doc,
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
        doc_id: doc,
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
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetRelationship: #{
            edit_target: edit_target,
            path: path,
            name: name,
            targets: targets,
        }},
    })
}

/// Set one USD connection in an explicit authored layer. The typed command
/// rejects missing source prims/properties, mismatched types, and invalid
/// runtime port paths; poll command_result(id) when the result is pending.
fn connection(doc, edit_target, path, name, type_name, sources, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
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
        doc_id: doc,
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
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetVariantSelection: #{
            edit_target: edit_target,
            path: path,
            variant_set: variant_set,
            variant: selection,
        }},
    })
}

/// Set the payload list on one exact prim through the standard USD payload op.
/// An empty list explicitly clears that layer's payload opinion.
fn payload(doc, edit_target, path, asset_paths, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetPayload: #{
            edit_target: edit_target,
            path: path,
            asset_paths: asset_paths,
        }},
    })
}

/// Author one typed USD reference list-op on an existing prim. Each entry is
/// #{ asset_path: "lunco://…", prim_path: () } (or an explicit absolute prim
/// path); use `"Explicit"` with an empty list to clear this layer's opinion.
/// Prepend/Append/Add/Delete preserve weaker-layer arcs according to USD list
/// composition. Resolve the target first so composed-only prims are rejected
/// visibly rather than guessed.
fn references(doc, edit_target, path, references, list_op, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetReferenceArcs: #{
            edit_target: edit_target,
            path: path,
            references: references,
            list_op: list_op,
        }},
    })
}

/// Set the stage root defaultPrim in one authored layer. Accepts either
/// "/World" or "World"; pass () to clear only that layer's opinion.
fn default_prim(doc, edit_target, path, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetDefaultPrim: #{
            edit_target: edit_target,
            default_prim: path,
        }},
    })
}

/// Set the standard USD kind metadata on one prim. Pass () to clear only the
/// selected layer's opinion and reveal any weaker kind.
fn prim_kind(doc, edit_target, path, kind, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetPrimKind: #{
            edit_target: edit_target,
            path: path,
            kind: kind,
        }},
    })
}

/// Activate or deactivate one exact prim without deleting its authored subtree.
fn active(doc, edit_target, path, is_active, parent_gen) {
    cmd("ApplyUsdOp", #{
        doc_id: doc,
        parent_gen: parent_gen,
        op: #{ SetActive: #{
            edit_target: edit_target,
            path: path,
            active: is_active,
        }},
    })
}

/// Submit a caller-built typed USD plan as one journal and undo unit.
/// Every op must contain its own explicit `edit_target` as required by `UsdOp`.
fn batch(doc, label, ops, parent_gen) {
    cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: parent_gen,
        label: label,
        ops: ops,
    })
}

/// Format one explicit three-component USD literal from numeric components.
/// The generic SetAttribute op accepts USDA literals, so these helpers keep
/// callers from hand-assembling tuple syntax while leaving the type explicit.
fn tuple3_literal(values) {
    if values == () || values.len() != 3 { return (); }
    "(" + values[0].to_string() + ", " + values[1].to_string()
        + ", " + values[2].to_string() + ")"
}

/// Format one explicit four-component USD quaternion literal.
fn quat4_literal(values) {
    if values == () || values.len() != 4 { return (); }
    "(" + values[0].to_string() + ", " + values[1].to_string()
        + ", " + values[2].to_string() + ", " + values[3].to_string() + ")"
}

fn plan_error(message) {
    #{ ok: false, error: message, ops: [] }
}

/// Build the standard authored facts for a new rigid-body frame.
///
/// This returns typed operations for `batch` or `propose`; it does not mutate
/// a document. Position, shape children, collision ownership, and any
/// kinematic state remain explicit caller-owned operations. Mass, centre of
/// mass, and diagonal inertia are required so a body cannot be created with a
/// hidden physics default.
fn rigid_body_plan(edit_target, parent_path, name, mass, center_of_mass, diagonal_inertia) {
    if edit_target == () || parent_path == () || name == () {
        return plan_error("rigid_body_plan requires an edit target, parent path, and name");
    }
    if mass <= 0.0 {
        return plan_error("rigid_body_plan requires positive mass");
    }
    let com = tuple3_literal(center_of_mass);
    let inertia = tuple3_literal(diagonal_inertia);
    if com == () || inertia == () {
        return plan_error("rigid_body_plan requires three-component centre of mass and inertia");
    }

    let path = parent_path + "/" + name;
    #{
        ok: true,
        path: path,
        ops: [
            #{ AddPrim: #{
                edit_target: edit_target,
                parent_path: parent_path,
                name: name,
                type_name: "Xform",
                reference: (),
                reference_prim_path: "",
            }},
            #{ SetApiSchemas: #{
                edit_target: edit_target,
                path: path,
                schemas: ["PhysicsRigidBodyAPI", "PhysicsMassAPI"],
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:mass",
                type_name: "float",
                value: mass.to_string(),
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:centerOfMass",
                type_name: "point3f",
                value: com,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:diagonalInertia",
                type_name: "float3",
                value: inertia,
            }},
        ],
    }
}

/// Build a fully framed, bounded standard USD revolute joint.
///
/// Body paths, both local anchors, both local frame quaternions, axis, limits,
/// and collision policy are required arguments. An explicit operation plan is
/// returned for review or one grouped journal commit; no Euler rotation or
/// implicit body relationship is introduced.
fn revolute_joint_plan(edit_target, parent_path, name, body0, body1, axis,
    local_pos0, local_pos1, local_rot0, local_rot1, lower_limit, upper_limit,
    collision_enabled) {
    if edit_target == () || parent_path == () || name == ()
        || body0 == () || body1 == () {
        return plan_error("revolute_joint_plan requires edit target, parent, name, and two body paths");
    }
    if axis != "X" && axis != "Y" && axis != "Z" {
        return plan_error("revolute_joint_plan axis must be X, Y, or Z");
    }
    if lower_limit == () || upper_limit == () || lower_limit > upper_limit {
        return plan_error("revolute_joint_plan requires ordered angular limits in degrees");
    }
    let pos0 = tuple3_literal(local_pos0);
    let pos1 = tuple3_literal(local_pos1);
    let rot0 = quat4_literal(local_rot0);
    let rot1 = quat4_literal(local_rot1);
    if pos0 == () || pos1 == () || rot0 == () || rot1 == () {
        return plan_error("revolute_joint_plan requires two positions and two quaternions");
    }

    let path = parent_path + "/" + name;
    #{
        ok: true,
        path: path,
        ops: [
            #{ AddPrim: #{
                edit_target: edit_target,
                parent_path: parent_path,
                name: name,
                type_name: "PhysicsRevoluteJoint",
                reference: (),
                reference_prim_path: "",
            }},
            #{ SetRelationship: #{
                edit_target: edit_target,
                path: path,
                name: "physics:body0",
                targets: [body0],
            }},
            #{ SetRelationship: #{
                edit_target: edit_target,
                path: path,
                name: "physics:body1",
                targets: [body1],
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:axis",
                type_name: "token",
                value: "\"" + axis + "\"",
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localPos0",
                type_name: "point3f",
                value: pos0,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localPos1",
                type_name: "point3f",
                value: pos1,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localRot0",
                type_name: "quatf",
                value: rot0,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localRot1",
                type_name: "quatf",
                value: rot1,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:lowerLimit",
                type_name: "float",
                value: lower_limit.to_string(),
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:upperLimit",
                type_name: "float",
                value: upper_limit.to_string(),
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:collisionEnabled",
                type_name: "bool",
                value: collision_enabled.to_string(),
            }},
        ],
    }
}

/// Build a fully framed standard USD fixed joint.
///
/// A fixed joint is still an authored relationship, not a hierarchy shortcut:
/// both body paths, both local anchors, both local frame quaternions, and the
/// collision policy are explicit. The plan is pure and can be reviewed or
/// grouped with reference/placement operations by a dynamic Rhai recipe.
fn fixed_joint_plan(edit_target, parent_path, name, body0, body1,
    local_pos0, local_pos1, local_rot0, local_rot1, collision_enabled) {
    if edit_target == () || parent_path == () || name == ()
        || body0 == () || body1 == () {
        return plan_error("fixed_joint_plan requires edit target, parent, name, and two body paths");
    }
    if !parent_path.starts_with("/") || (parent_path.ends_with("/") && parent_path != "/") {
        return plan_error("fixed_joint_plan parent must be an absolute USD path");
    }
    if name == "" || name.contains("/") {
        return plan_error("fixed_joint_plan name must be one USD identifier");
    }
    if !body0.starts_with("/") || !body1.starts_with("/") || body0 == body1 {
        return plan_error("fixed_joint_plan requires two distinct absolute body paths");
    }
    let pos0 = tuple3_literal(local_pos0);
    let pos1 = tuple3_literal(local_pos1);
    let rot0 = quat4_literal(local_rot0);
    let rot1 = quat4_literal(local_rot1);
    if pos0 == () || pos1 == () || rot0 == () || rot1 == () {
        return plan_error("fixed_joint_plan requires two positions and two quaternions");
    }

    let path = if parent_path == "/" { "/" + name } else { parent_path + "/" + name };
    #{
        ok: true,
        path: path,
        ops: [
            #{ AddPrim: #{
                edit_target: edit_target,
                parent_path: parent_path,
                name: name,
                type_name: "PhysicsFixedJoint",
                reference: (),
                reference_prim_path: "",
            }},
            #{ SetRelationship: #{
                edit_target: edit_target,
                path: path,
                name: "physics:body0",
                targets: [body0],
            }},
            #{ SetRelationship: #{
                edit_target: edit_target,
                path: path,
                name: "physics:body1",
                targets: [body1],
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localPos0",
                type_name: "point3f",
                value: pos0,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localPos1",
                type_name: "point3f",
                value: pos1,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localRot0",
                type_name: "quatf",
                value: rot0,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:localRot1",
                type_name: "quatf",
                value: rot1,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:collisionEnabled",
                type_name: "bool",
                value: collision_enabled.to_string(),
            }},
        ],
    }
}

/// Validate and queue an explicit proposal without mutating the document.
/// `scope` is one of `SourceAsset`, `Assembly`, or `InstanceOverride`; the
/// proposal must carry the generation the caller inspected.
fn propose(doc, scope, label, ops, parent_gen) {
    cmd("CreateUsdProposal", #{
        doc_id: doc,
        scope: scope,
        label: label,
        parent_gen: parent_gen,
        ops: ops,
    })
}

/// Inspect pending proposals, their typed operations, diagnostics, and stale
/// state for one explicit document.
fn review_session(doc) {
    query("InspectUsdEditSession", #{ doc_id: doc })
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
    cmd("AttachComponent", #{ doc_id: doc, spec: spec })
}

/// Detach a component through the existing topology validator and change-set
/// path. The spec must identify the exact component, joint, and socket paths.
fn detach_component(doc, spec) {
    cmd("DetachComponent", #{ doc_id: doc, spec: spec })
}

/// Attach a source-backed program through the existing USD program contract.
/// Build `spec` with the canonical program-input/output constructors in this
/// library; this wrapper only supplies the explicit document identity and
/// dispatches the typed command.
fn attach_program(doc, spec) {
    cmd("AttachProgram", #{ doc_id: doc, spec: spec })
}

/// Undo the latest history group on one explicit document.
fn undo(doc) {
    cmd("UndoDocument", #{ doc_id: doc })
}

/// Redo the latest undone history group on one explicit document.
fn redo(doc) {
    cmd("RedoDocument", #{ doc_id: doc })
}
