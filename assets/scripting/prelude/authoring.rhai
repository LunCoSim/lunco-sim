// Script-first robot and mission authoring.
//
// This file is deliberately a thin policy layer over the existing typed command
// and query surfaces. USD remains the authority for scene topology, placement,
// joints and metadata. Modelica remains the authority for equations and
// connections. Every document edit takes an explicit document id so a script
// cannot silently write to whichever editor tab happens to be active.

fn __missing_document(kind) {
    #{ ok: false, error: "missing explicit " + kind + " document id" }
}

// ── USD assembly ───────────────────────────────────────────────────────────

/// Apply one USD operation to an explicit document.
fn usd_apply(doc, op) {
    if doc == () { return __missing_document("USD"); }
    cmd("ApplyUsdOp", #{ doc: doc, op: op })
}

/// Apply an ordered USD change set as one journal/undo unit.
fn usd_apply_ops(doc, label, ops) {
    if doc == () { return __missing_document("USD"); }
    cmd("ApplyUsdOps", #{ doc: doc, label: label, ops: ops })
}

/// Add a referenced component or a typed primitive under `parent_path`.
/// `type_name` and `reference` may be `()` when the field is not needed.
fn usd_add_prim(doc, edit_target, parent_path, name, type_name, reference) {
    usd_apply(doc, #{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent_path,
        name: name,
        type_name: type_name,
        reference: reference,
    }})
}

/// Set a prim's semantic USD-frame translation or rotation (degrees).
fn usd_set_translate(doc, edit_target, path, value) {
    usd_apply(doc, #{ SetTranslate: #{
        edit_target: edit_target, path: path, value: value
    }})
}

fn usd_set_rotate(doc, edit_target, path, value) {
    usd_apply(doc, #{ SetRotate: #{
        edit_target: edit_target, path: path, value: value
    }})
}

/// Set an arbitrary USD attribute. For `type_name == "string"`, `value` is
/// raw string content; all other values use USDA literal syntax.
fn usd_set_attribute(doc, edit_target, path, name, type_name, value) {
    usd_apply(doc, #{ SetAttribute: #{
        edit_target: edit_target,
        path: path,
        name: name,
        type_name: type_name,
        value: value,
    }})
}

fn usd_set_relationship(doc, edit_target, path, name, targets) {
    usd_apply(doc, #{ SetRelationship: #{
        edit_target: edit_target, path: path, name: name, targets: targets
    }})
}

fn usd_set_connection(doc, edit_target, path, name, type_name, sources) {
    usd_apply(doc, #{ SetConnection: #{
        edit_target: edit_target,
        path: path,
        name: name,
        type_name: type_name,
        sources: sources,
    }})
}

fn usd_set_api_schemas(doc, edit_target, path, schemas) {
    usd_apply(doc, #{ SetApiSchemas: #{
        edit_target: edit_target, path: path, schemas: schemas
    }})
}

/// Attach a source-backed simulation program through the authoritative USD
/// command. `inputs` is an array of `program_input_*` maps and `outputs` is an
/// array of `program_output` maps. Use `@root@` for authored scene content or
/// `@runtime@` for a document-backed runtime overlay; choose explicitly.
fn attach_program(doc, edit_target, host_path, name, source_asset, inputs, outputs, realtime_safe) {
    if doc == () { return __missing_document("USD"); }
    cmd("AttachProgram", #{
        doc: doc,
        spec: #{
            edit_target: edit_target,
            host_path: host_path,
            name: name,
            source_asset: source_asset,
            inputs: inputs,
            outputs: outputs,
            realtime_safe: realtime_safe,
        },
    })
}

/// Declare a connected scalar program input.
fn program_input_connection(name, source) {
    #{
        name: name,
        type_name: "float",
        default_value: (),
        connection: source,
    }
}

/// Declare a constant scalar program input.
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

fn usd_select_variant(doc, edit_target, path, variant_set, variant) {
    usd_apply(doc, #{ SetVariantSelection: #{
        edit_target: edit_target,
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
fn attach_fixed(doc, edit_target, host_path, name, joint_name, asset, placement) {
    if doc == () { return __missing_document("USD"); }
    cmd("AttachComponent", #{
        doc: doc,
        spec: #{
            edit_target: edit_target,
            host_path: host_path,
            name: name,
            joint_name: joint_name,
            asset: asset,
            placement: placement,
            rotate_deg: [0.0, 0.0, 0.0],
            joint: "Fixed",
        },
    })
}

fn attach_revolute(doc, edit_target, host_path, name, joint_name, asset, placement, rotate_deg, axis) {
    if doc == () { return __missing_document("USD"); }
    cmd("AttachComponent", #{
        doc: doc,
        spec: #{
            edit_target: edit_target,
            host_path: host_path,
            name: name,
            joint_name: joint_name,
            asset: asset,
            placement: placement,
            rotate_deg: rotate_deg,
            joint: #{ Revolute: #{ axis: axis } },
        },
    })
}

fn attach_prismatic(doc, edit_target, host_path, name, joint_name, asset, placement, rotate_deg, axis) {
    if doc == () { return __missing_document("USD"); }
    cmd("AttachComponent", #{
        doc: doc,
        spec: #{
            edit_target: edit_target,
            host_path: host_path,
            name: name,
            joint_name: joint_name,
            asset: asset,
            placement: placement,
            rotate_deg: rotate_deg,
            joint: #{ Prismatic: #{ axis: axis } },
        },
    })
}

/// Remove one attached component, its exact generated joint, and optional
/// socket occupancy as one validated USD transaction.
fn detach_component(doc, edit_target, component_path, joint_path, socket_path) {
    if doc == () { return __missing_document("USD"); }
    cmd("DetachComponent", #{
        doc: doc,
        spec: #{
            edit_target: edit_target,
            component_path: component_path,
            joint_path: joint_path,
            socket_path: socket_path,
        },
    })
}

// ── Modelica graph authoring ───────────────────────────────────────────────

fn modelica_apply(doc, ops) {
    if doc == () { return __missing_document("Modelica"); }
    cmd("ApplyModelicaOps", #{ doc: doc, ops: ops })
}

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

// Scenario scripts are attached to host entities, so checkpoint state is
// authored on that host prim. The mission supplies its USD document explicitly
// through `fn mission_document(me)`. This keeps writes correct for referenced
// prims and works with the existing typed command surface without a Rust rebuild.
// Empty string is the logical tombstone used by mission_checkpoint_clear.
fn mission_checkpoint(me, doc, edit_target, key, value) {
    let path = name(me);
    if path == () || doc == () { return false; }
    let result = usd_set_attribute(
        doc, edit_target, path, "lunco:mission:checkpoint:" + key, "string", value
    );
    result.ok == true
}

fn mission_checkpoint_read(me, key) {
    let path = name(me);
    if path == () { return (); }
    let attr = "lunco:mission:checkpoint:" + key;
    let result = query("QueryUsdPrim", #{ path: path, attrs: [attr] });
    if result == () || result.attrs == () || result.attrs[attr] == () { return (); }
    if result.attrs[attr] == "" { return (); }
    result.attrs[attr]
}

fn mission_checkpoint_clear(me, doc, edit_target, key) {
    mission_checkpoint(me, doc, edit_target, key, "")
}
