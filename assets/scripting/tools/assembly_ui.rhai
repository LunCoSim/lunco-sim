// Reusable Assembly Editor presentation policy.
//
// This library composes the existing workbench panels for one explicit USD
// preview session. It owns no document, selection, layout, or edit state:
// `assembly_edit` and the typed command/query owners remain authoritative.

// Stable surface templates are data for agents and authored workflows. The
// workbench resolves the registered panel ids; section/workflow entries point
// at the existing owner and do not pretend to be independent panels.
fn panel_roles() {
    [
        "browser",
        "structure",
        "preview",
        "inspector",
        "connections",
        "animation",
        "mount",
        "review",
        "persistence",
    ]
}

fn surface_kind(role) {
    if role == "animation" || role == "mount" || role == "review" {
        "section"
    } else if role == "persistence" {
        "workflow"
    } else {
        "panel"
    }
}

fn surface_section(role) {
    if role == "animation" { "animation" }
    else if role == "mount" { "mount" }
    else if role == "review" { "review" }
    else { () }
}

fn surface_capabilities(role) {
    if role == "browser" { ["open_usd"] }
    else if role == "structure" { ["inspect_prim", "select_prim"] }
    else if role == "preview" { ["visual_checkpoint", "open_views"] }
    else if role == "inspector" {
        ["transform", "attribute", "variant", "joint", "animation"]
    } else if role == "connections" { ["connection"] }
    else if role == "animation" { ["transport", "keyframe"] }
    else if role == "mount" { ["attach_component", "detach_component"] }
    else if role == "review" { ["propose", "review", "commit"] }
    else if role == "persistence" { ["save", "save_as", "discard", "close"] }
    else { [] }
}

fn panel_template(role, preview, doc, edit_target) {
    let panel = panel_id(role);
    if panel == () && role != "persistence" {
        return #{ ok: false, error: "unknown assembly editor surface role", role: role };
    }
    let template = #{
        role: role,
        kind: surface_kind(role),
        preview: preview,
        doc: doc,
        edit_target: edit_target,
        capabilities: surface_capabilities(role),
    };
    if panel != () {
        template.panel = panel;
    }
    let section = surface_section(role);
    if section != () {
        template.section = section;
    }
    template
}

fn panel_templates(preview, doc, edit_target) {
    let templates = [];
    for role in panel_roles() {
        templates.push(panel_template(role, preview, doc, edit_target));
    }
    templates
}

fn panel_id(role) {
    if role == "browser" { "lunco.workbench.twin_browser" }
    else if role == "structure" { "usd_prim_tree" }
    else if role == "preview" { "usd::viewport" }
    else if role == "inspector" { "sandbox_inspector" }
    else if role == "connections" { "usd_connection_canvas" }
    else if role == "animation" { "sandbox_environment" }
    else if role == "mount" || role == "review" { "sandbox_inspector" }
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
    let result = if role == "preview" {
        cmd("FocusUsdPreview", #{ preview: preview })
    } else {
        cmd("FocusPanel", #{ id: panel })
    };
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
       viewport: viewport, templates: panel_templates(preview, doc, edit_target),
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
    focus("animation", preview, doc, edit_target)
}

fn open_mount(preview, doc, edit_target) {
    focus("mount", preview, doc, edit_target)
}

fn open_review(preview, doc, edit_target) {
    focus("review", preview, doc, edit_target)
}

/// Select one prim in the focused preview through the shared typed selection
/// owner. Focusing first makes the command's presentation precondition
/// explicit and keeps hidden previews from mutating the visible Inspector.
fn select_prim(preview, path, extend, toggle) {
    let focused = cmd("FocusUsdPreview", #{ preview: preview });
    if focused.ok == false {
        return #{ ok: false, focus: focused };
    }
    let selected = cmd("SelectUsdPrim", #{
        preview: preview,
        path: path,
        extend: extend,
        toggle: toggle,
    });
    #{ ok: selected.ok, focus: focused, selection: selected }
}
