// Reusable Assembly Editor presentation policy.
//
// This library composes the existing workbench panels for one explicit USD
// preview session. It owns no document, selection, layout, or edit state:
// `assembly_edit` and the typed command/query owners remain authoritative.

// Stable panel templates are data for agents and authored workflows. The
// workbench resolves these registered panel ids; no Rust layout is generated
// and no panel state is copied into Rhai.
fn panel_template(role, preview, doc, edit_target) {
    let template = #{
        role: role,
        panel: panel_id(role),
        preview: preview,
        doc: doc,
        edit_target: edit_target,
    };
    if role == "inspector" {
        template.sections = ["selection", "mount", "review"];
    } else if role == "environment" {
        template.sections = ["animation"];
    }
    template
}

fn panel_templates(preview, doc, edit_target) {
    [
        panel_template("browser", preview, doc, edit_target),
        panel_template("structure", preview, doc, edit_target),
        panel_template("preview", preview, doc, edit_target),
        panel_template("inspector", preview, doc, edit_target),
        panel_template("connections", preview, doc, edit_target),
        panel_template("environment", preview, doc, edit_target),
    ]
}

fn panel_id(role) {
    if role == "browser" { "lunco.workbench.twin_browser" }
    else if role == "structure" { "usd_prim_tree" }
    else if role == "preview" { "usd::viewport" }
    else if role == "inspector" { "sandbox_inspector" }
    else if role == "connections" { "usd_connection_canvas" }
    else if role == "environment" { "sandbox_environment" }
    else { () }
}

// Focus an existing panel in the explicit session's Editor workspace. The
// preview/doc arguments are retained in the returned descriptor so callers
// cannot accidentally treat a panel selection as a document identity.
fn focus(role, preview, doc, edit_target) {
    let panel = panel_id(role);
    if panel == () {
        return #{ ok: false, error: "unknown assembly editor panel role", role: role };
    }
    let result = cmd("FocusPanel", #{ id: panel });
    #{ ok: result.ok, role: role, panel: panel, preview: preview, doc: doc, edit_target: edit_target, command: result }
}

// Open one complete Editor workspace. Repeating this with another preview id
// keeps both preview leases independent; the USD owner rejects reused or
// unresolved identities instead of this library guessing.
fn open_session(preview, doc, edit_target) {
    let perspective = cmd("ActivatePerspective", #{ id: "editor" });
    if perspective.ok == false {
        return #{ ok: false, error: "Editor perspective could not be activated", perspective: perspective };
    }
    let opened = assembly_edit::preview_open(preview, doc, edit_target);
    if opened.ok == false {
        return #{ ok: false, error: "USD preview admission failed", perspective: perspective, preview_open: opened };
    }
    let viewport = focus("preview", preview, doc, edit_target);
    #{ ok: viewport.ok,
       preview: preview, doc: doc, edit_target: edit_target,
       perspective: perspective,
       viewport: viewport, panels: panel_templates(preview, doc, edit_target),
       note: "Preview admission is complete; panel view-models populate from the explicit session." }
}

fn open_structure(preview, doc, edit_target) {
    focus("structure", preview, doc, edit_target)
}

fn open_preview(preview, doc, edit_target) {
    focus("preview", preview, doc, edit_target)
}

fn open_inspector(preview, doc, edit_target) {
    focus("inspector", preview, doc, edit_target)
}

fn open_connections(preview, doc, edit_target) {
    focus("connections", preview, doc, edit_target)
}

fn open_animation(preview, doc, edit_target) {
    focus("environment", preview, doc, edit_target)
}
