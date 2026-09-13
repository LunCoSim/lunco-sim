// Generic authoring evidence policy.
//
// USD remains the authority for topology, materials, frames, provenance, and
// authored values. This library only composes the existing typed queries and
// presentation commands into an actionable authoring workflow. It contains no
// vehicle names, no second graph, and no Rust-side product policy.

import "assembly_edit" as assembly_edit;

fn query_ok(record) {
    record != () && record.ok != false
}

fn failure(kind, message) {
    #{ ok: false, kind: kind, error: message, errors: [message] }
}

fn inspect_record(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        children: true,
        attrs: [],
        rels: [],
        topology: true,
        collision_bounds: true,
        connections: true,
        schemas: true,
    });
    if query_ok(record) { return record; }
    // InspectUsdDocument is the authored/composed existence owner. Use it to
    // represent a real create/delete without confusing an unavailable stage
    // or stale projection with an absent prim.
    let document = query("InspectUsdDocument", #{ doc_id: doc, path: path });
    if query_ok(document) && document.prim != () && document.prim.exists == false {
        return #{ ok: true, exists: false, path: path };
    }
    record
}

fn comparable(record) {
    let exists = query_ok(record);
    if !exists {
        return #{ exists: false, type_name: "", attrs: [], relationships: [], connections: [], api_schemas: [], topology: (), collision_bounds: (), world_position: () };
    }
    #{
        exists: true,
        type_name: record.type_name,
        children: record.children,
        attrs: record.attrs,
        relationships: record.relationships,
        connections: record.connections,
        api_schemas: record.api_schemas,
        topology: record.topology,
        collision_bounds: record.collision_bounds,
        world_position: record.world_position,
    }
}

fn changed_fields(before, after) {
    if before.exists != after.exists {
        return [#{ field: "exists", before: before.exists, after: after.exists }];
    }
    if !before.exists {
        return [];
    }
    let changes = [];
    let fields = [
        "exists", "type_name", "children", "attrs", "relationships",
        "connections", "api_schemas", "topology", "collision_bounds",
        "world_position",
    ];
    for field in fields {
        let old_value = before[field];
        let new_value = after[field];
        if old_value.to_string() != new_value.to_string() {
            changes.push(#{ field: field, before: old_value, after: new_value });
        }
    }
    changes
}

fn consequences(changes) {
    let material = false;
    let collision = false;
    let joints = false;
    let frames = false;
    let ports = false;
    let topology = false;
    for change in changes {
        if change.field == "attrs" {
            material = true;
            frames = true;
        }
        if change.field == "collision_bounds" || change.field == "api_schemas" {
            collision = true;
        }
        if change.field == "topology" {
            joints = true;
            topology = true;
        }
        if change.field == "connections" {
            ports = true;
        }
        if change.field == "children" || change.field == "exists" {
            topology = true;
        }
    }
    #{
        material: material,
        collision: collision,
        joints: joints,
        frames: frames,
        ports: ports,
        topology: topology,
    }
}

/// Compare exact paths from two forked USD documents. The caller normally
/// supplies `InspectUsdEditSession.proposals[0].affected_paths`; requiring
/// paths keeps the operation bounded and makes identity explicit.
fn candidate_diff(before_doc, after_doc, paths) {
    if before_doc == () || after_doc == () {
        return failure("candidate_diff", "candidate_diff requires two document ids");
    }
    if paths == () || paths.len() == 0 {
        return failure("candidate_diff", "candidate_diff requires affected USD paths");
    }
    let entries = [];
    let errors = [];
    for path in paths {
        if type_of(path) != "string" || !path.starts_with("/") {
            errors.push("invalid USD path in candidate diff");
            continue;
        }
        let before_record = inspect_record(before_doc, path);
        let after_record = inspect_record(after_doc, path);
        if !query_ok(before_record) || !query_ok(after_record) {
            errors.push(path + ": one candidate document is not ready for inspection");
        }
        let old_value = comparable(before_record);
        let new_value = comparable(after_record);
        let changes = changed_fields(old_value, new_value);
        entries.push(#{
            path: path,
            changed: changes.len() > 0,
            changes: changes,
            consequences: consequences(changes),
        });
    }

    // Lint is a document-scoped owner. The command is deliberately separate
    // from the diff; a queued report must remain visible as pending rather
    // than being represented as a guessed clean result.
    let lint_command = cmd("RunLint", #{
        domain: "usd",
        scope: "loaded_stages",
        policy: "warn",
        doc_id: after_doc,
    });
    let lint = query("LintReport", #{ doc_id: after_doc });
    #{
        ok: errors.len() == 0 && query_ok(lint_command),
        kind: "candidate_diff",
        before_doc: before_doc,
        after_doc: after_doc,
        paths: entries,
        errors: errors,
        lint: lint,
        lint_pending: lint == () || lint.pending == true || lint.stale == true,
        command: lint_command,
    }
}

fn owner_for_finding(finding) {
    let domain = if finding.domain != () { finding.domain } else { "" };
    let rule = if finding.rule != () { finding.rule } else { "" };
    if domain == "usd" && (rule.contains("composition") || rule.contains("source")) {
        return "Composition";
    }
    if domain == "usd" && (rule.contains("joint") || rule.contains("collider")
        || rule.contains("body") || rule.contains("physics")) {
        return "USD/Physics";
    }
    if domain == "modelica" { return "Modelica"; }
    if domain == "rhai" { return "Rhai"; }
    if domain == "presentation" || rule.contains("material") || rule.contains("camera") {
        return "Presentation";
    }
    if domain == "" { "Unclassified" } else { domain }
}

/// Group findings by the owning subsystem and severity while retaining every
/// raw finding and its exact subject. The grouping is presentation policy, so
/// adding a domain or changing labels does not require a Rust rebuild.
fn group_diagnostics(findings) {
    let groups = [];
    for finding in findings {
        let owner = owner_for_finding(finding);
        let severity = if finding.severity != () { finding.severity } else { "warn" };
        let index = -1;
        for i in 0..groups.len() {
            if groups[i].owner == owner && groups[i].severity == severity {
                index = i;
                break;
            }
        }
        if index < 0 {
            groups.push(#{ owner: owner, severity: severity, findings: [finding] });
        } else {
            groups[index].findings.push(finding);
        }
    }
    #{ ok: true, kind: "diagnostic_groups", groups: groups, raw: findings }
}

/// Select/reveal/frame one exact diagnostic subject in one preview. Prim-tree
/// reveal is a consequence of the canonical selection owner; frame uses the
/// same preview lease and never searches the live scene by display name.
fn navigate_diagnostic(preview, view, finding) {
    if finding == () || finding.subject == () || !finding.subject.starts_with("/") {
        return failure("diagnostic_navigation", "finding has no absolute USD subject path");
    }
    let selected = cmd("SelectUsdPrim", #{
        preview: preview,
        path: finding.subject,
        extend: false,
        toggle: false,
    });
    if selected.ok == false {
        return #{ ok: false, selection: selected, error: "diagnostic subject could not be selected" };
    }
    let framed = assembly_edit::preview_view_frame_path(preview, view, finding.subject);
    #{ ok: framed.ok != false, selected: selected, framed: framed, path: finding.subject }
}

/// Build one unified inspection record over standard USD facts and optionally
/// enable the reusable runtime diagnostic layers. The returned record is the
/// UI/agent contract; visuals and provenance remain read-only evidence.
fn inspection_snapshot(doc, root, path) {
    let record = inspect_record(doc, path);
    if !query_ok(record) {
        return failure("inspection", path + ": composed inspection failed");
    }
    let source = query("InspectUsdDocument", #{ doc_id: doc, path: path });
    let collision = if record.collision_bounds == () {
        #{ present: false, reason: "no composed collision geometry" }
    } else {
        #{ present: true, bounds: record.collision_bounds }
    };
    let provenance = if source != () && source.prim != () {
        #{ present: true, references: source.prim.references }
    } else {
        #{ present: false, reason: "document provenance is unavailable" }
    };
    let parts = if record.topology != () && record.topology.parts != () {
        record.topology.parts
    } else {
        []
    };
    #{
        ok: true,
        kind: "inspection_snapshot",
        doc_id: doc,
        root: root,
        path: path,
        visual: #{ type_name: record.type_name, world_position: record.world_position,
            parts: parts },
        collision: #{ present: collision.present, bounds: collision.bounds,
            parts: parts },
        joints: if record.topology != () { record.topology.joints } else { [] },
        frames: if record.topology != () { record.topology.frames } else { [] },
        materials: if parts.len() > 0 { parts } else { record.attrs },
        provenance: provenance,
        schemas: record.api_schemas,
        connections: record.connections,
    }
}

/// Return the generic overlay commands used by the unified inspection mode.
/// Callers choose when to enable/disable the mode; no per-frame Rust policy is
/// introduced here.
fn inspection_layers(enabled) {
    cmd("SetDiagnosticLayers", #{
        enabled: enabled,
        layers: ["joints", "physics_frames", "physics_mass", "physics_forces", "wheel_forces"],
    })
}

fn inspection_mode(doc, root, path, enabled) {
    let snapshot = inspection_snapshot(doc, root, path);
    if !snapshot.ok { return snapshot; }
    let layers = inspection_layers(enabled);
    snapshot.layers = layers;
    snapshot
}
