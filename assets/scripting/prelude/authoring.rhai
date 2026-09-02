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

/// Spawn a catalog entry. Catalog spawning is the authoritative replicated
/// path; callers do not fabricate ECS entities or USD runtime identities.
fn spawn_robot(entry_id, position) {
    cmd("SpawnEntity", #{ entry_id: entry_id, position: position })
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
    let path = usd_path(me);
    if path == () || doc == () { return false; }
    let result = cmd("ApplyUsdOp", #{
        doc: doc,
        op: #{ SetAttribute: #{
            edit_target: edit_target,
            path: path,
            name: "lunco:mission:checkpoint:" + key,
            type_name: "string",
            value: value,
        }},
    });
    result.ok == true
}

fn mission_checkpoint_read(me, key) {
    let path = usd_path(me);
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
