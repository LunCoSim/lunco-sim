// Script-first robot and mission authoring.
//
// This file is deliberately a thin policy layer over the existing typed command
// and query surfaces. USD remains the authority for scene topology, placement,
// joints and metadata; Modelica remains the authority for equations and
// connections. These helpers only make those contracts pleasant to use from a
// hot-reloadable script.

// ── Document selection ─────────────────────────────────────────────────────

// Return the active document only when it belongs to the requested domain.
// Refusing to guess a document is important: a mission should never write USD
// ops into the active Modelica document (or vice versa).
fn __active_document(kind) {
    let listing = query("ListOpenDocuments", #{});
    if listing == () || listing.active_doc_id == () { return (); }
    for d in listing.open_documents {
        if d.active == true && d.kind == kind { return d.doc_id; }
    }
    ()
}

/// The active USD document, or `()` when the active tab is another domain.
fn usd_document() { __active_document("usd") }

/// The active Modelica document, or `()` when the active tab is another domain.
fn modelica_document() { __active_document("modelica") }

fn __missing_document(kind) {
    #{ ok: false, error: "no active " + kind + " document" }
}

// ── USD assembly ───────────────────────────────────────────────────────────

/// Apply one USD operation to an explicit document.
fn usd_apply_in(doc, op) {
    if doc == () { return __missing_document("USD"); }
    cmd("ApplyUsdOp", #{ doc: doc, op: op })
}

/// Apply one USD operation to the active USD document.
fn usd_apply(op) { usd_apply_in(usd_document(), op) }

/// Apply an ordered USD change set to an explicit document. The command keeps
/// the batch as one journal/undo unit and validates the whole intent first.
fn usd_apply_ops_in(doc, label, ops) {
    if doc == () { return __missing_document("USD"); }
    cmd("ApplyUsdOps", #{ doc: doc, label: label, ops: ops })
}

/// Apply an ordered USD change set to the active USD document.
fn usd_apply_ops(label, ops) { usd_apply_ops_in(usd_document(), label, ops) }

/// Add a referenced component or a typed primitive under `parent_path`.
/// `type_name` and `reference` may be `()` when the field is not needed.
fn usd_add_prim_in(doc, parent_path, name, type_name, reference) {
    usd_apply_in(doc, #{ AddPrim: #{
        edit_target: "@root@",
        parent_path: parent_path,
        name: name,
        type_name: type_name,
        reference: reference,
    }})
}

fn usd_add_prim(parent_path, name, type_name, reference) {
    usd_add_prim_in(usd_document(), parent_path, name, type_name, reference)
}

/// Set a prim's semantic USD-frame translation or rotation (degrees).
fn usd_set_translate_in(doc, path, value) {
    usd_apply_in(doc, #{ SetTranslate: #{
        edit_target: "@root@", path: path, value: value
    }})
}
fn usd_set_translate(path, value) { usd_set_translate_in(usd_document(), path, value) }

fn usd_set_rotate_in(doc, path, value) {
    usd_apply_in(doc, #{ SetRotate: #{
        edit_target: "@root@", path: path, value: value
    }})
}
fn usd_set_rotate(path, value) { usd_set_rotate_in(usd_document(), path, value) }

/// Set an arbitrary USD attribute. For `type_name == "string"`, `value` is
/// raw string content; all other values use USDA literal syntax.
fn usd_set_attribute_in(doc, path, name, type_name, value) {
    usd_apply_in(doc, #{ SetAttribute: #{
        edit_target: "@root@",
        path: path,
        name: name,
        type_name: type_name,
        value: value,
    }})
}
fn usd_set_attribute(path, name, type_name, value) {
    usd_set_attribute_in(usd_document(), path, name, type_name, value)
}

fn usd_set_relationship_in(doc, path, name, targets) {
    usd_apply_in(doc, #{ SetRelationship: #{
        edit_target: "@root@", path: path, name: name, targets: targets
    }})
}
fn usd_set_relationship(path, name, targets) {
    usd_set_relationship_in(usd_document(), path, name, targets)
}

fn usd_set_connection_in(doc, path, name, type_name, sources) {
    usd_apply_in(doc, #{ SetConnection: #{
        edit_target: "@root@",
        path: path,
        name: name,
        type_name: type_name,
        sources: sources,
    }})
}
fn usd_set_connection(path, name, type_name, sources) {
    usd_set_connection_in(usd_document(), path, name, type_name, sources)
}

fn usd_set_api_schemas(path, schemas) {
    usd_apply(#{ SetApiSchemas: #{
        edit_target: "@root@", path: path, schemas: schemas
    }})
}

fn usd_select_variant(path, variant_set, variant) {
    usd_apply(#{ SetVariantSelection: #{
        edit_target: "@root@",
        path: path,
        variant_set: variant_set,
        variant: variant,
    }})
}

/// Spawn a catalog entry. Catalog spawning is the authoritative replicated
/// path; callers do not fabricate ECS entities or USD runtime identities.
fn spawn_robot(entry_id, position) {
    cmd("SpawnEntity", #{ entry_id: entry_id, position: position })
}

/// Attach a referenced component with one derived joint anchor.
fn attach_fixed(host_path, name, asset, placement) {
    cmd("AttachComponent", #{
        doc: usd_document(),
        spec: #{
            edit_target: "@root@",
            host_path: host_path,
            name: name,
            asset: asset,
            placement: placement,
            rotate_deg: [0.0, 0.0, 0.0],
            joint: "Fixed",
        },
    })
}

fn attach_revolute(host_path, name, asset, placement, rotate_deg, axis) {
    cmd("AttachComponent", #{
        doc: usd_document(),
        spec: #{
            edit_target: "@root@",
            host_path: host_path,
            name: name,
            asset: asset,
            placement: placement,
            rotate_deg: rotate_deg,
            joint: #{ Revolute: #{ axis: axis } },
        },
    })
}

fn attach_prismatic(host_path, name, asset, placement, rotate_deg, axis) {
    cmd("AttachComponent", #{
        doc: usd_document(),
        spec: #{
            edit_target: "@root@",
            host_path: host_path,
            name: name,
            asset: asset,
            placement: placement,
            rotate_deg: rotate_deg,
            joint: #{ Prismatic: #{ axis: axis } },
        },
    })
}

// ── Modelica graph authoring ───────────────────────────────────────────────

fn modelica_apply_in(doc, ops) {
    if doc == () { return __missing_document("Modelica"); }
    cmd("ApplyModelicaOps", #{ doc: doc, ops: ops })
}
fn modelica_apply(ops) { modelica_apply_in(modelica_document(), ops) }

fn modelica_placement(x, y, width, height) {
    #{ x: x, y: y, width: width, height: height }
}

fn modelica_modification(name, value) { #{ name: name, value: value } }

fn modelica_add_component(class, type_name, name, placement, modifications) {
    #{ AddComponent: #{
        class: class,
        type_name: type_name,
        name: name,
        modifications: modifications,
        placement: placement,
    }}
}

fn modelica_connect(class, from_component, from_port, to_component, to_port) {
    #{ AddConnection: #{
        class: class,
        from_component: from_component,
        from_port: from_port,
        to_component: to_component,
        to_port: to_port,
        line_points: [],
    }}
}

fn modelica_set_parameter(class, component, param, value) {
    #{ SetParameter: #{
        class: class, component: component, param: param, value: value
    }}
}

// ── Durable mission checkpoints ────────────────────────────────────────────

// Mission state lives on the program prim, not in a second runtime database.
// The write is journaled, authored, and visible to normal USD inspection. Use
// it at phase transitions/objective completion, not on every tick.
fn mission_checkpoint(me, key, value) {
    let path = name(me);
    if path == () { return false; }
    let result = usd_set_attribute(path, "lunco:mission:checkpoint:" + key, "string", value);
    result.ok == true
}

/// Explicit-document form for a mission whose active editor tab is not USD.
/// Prefer this in long-running missions: the active-tab convenience above is
/// intentionally conservative and refuses to guess a document.
fn mission_checkpoint_in(me, doc, key, value) {
    let path = name(me);
    if path == () { return false; }
    let result = usd_set_attribute_in(
        doc, path, "lunco:mission:checkpoint:" + key, "string", value
    );
    result.ok == true
}

fn mission_checkpoint_read(me, key) {
    let path = name(me);
    if path == () { return (); }
    let result = query("QueryUsdPrim", #{
        path: path,
        attrs: ["lunco:mission:checkpoint:" + key],
    });
    if result == () || result.attrs == () { return (); }
    result.attrs["lunco:mission:checkpoint:" + key]
}

fn mission_checkpoint_clear(me, key) {
    mission_checkpoint(me, key, "")
}
