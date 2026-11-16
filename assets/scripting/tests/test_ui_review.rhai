// Runtime observations for the workbench UI review gate. The native driver owns
// gestures and waits; these assertions observe production command/query owners.
fn ui_panel_visible(id) {
    let found = false;
    for panel in query("InspectWorkbench").rendered_panels {
        if panel.id == id { found = panel.width > 0 && panel.height > 0; }
    }
    report_verdict([t_true(found, "requested panel is rendered: " + id)], "UI panel " + id, "UI_REVIEW");
}
fn ui_titles_unique() {
    let seen = [];
    let unique = true;
    for panel in query("InspectWorkbench").panels {
        if panel.title != "" {
            if seen.contains(panel.title) { unique = false; }
            seen.push(panel.title);
        }
    }
    report_verdict([t_true(unique, "panel display names distinguish their tasks")], "UI panel names", "UI_REVIEW");
}
fn ui_draft_retained() {
    let editor = query("InspectBehaviourEditor");
    report_verdict([
        t_true(editor.source.contains("UI_DRAFT_SENTINEL"), "draft text survives selection round trip"),
        t_true(editor.dirty, "selection does not clear draft changes"),
    ], "UI draft retention", "UI_REVIEW");
}
fn ui_draft_rejected() {
    let editor = query("InspectBehaviourEditor");
    report_verdict([
        t_true(editor.source.contains("UI_DRAFT_SENTINEL"), "invalid draft remains editable"),
        t_true(editor.dirty && !editor.pending, "failed compile releases pending state without discarding the draft"),
        t_true(query("GetReadiness").application_available, "invalid source cannot crash the workbench"),
    ], "UI failed draft", "UI_REVIEW");
}
fn ui_drafts_retired() {
    let editor = query("InspectBehaviourEditor");
    report_verdict([
        t_true(editor.draft_count == 0, "scene teardown retires every draft"),
        t_true(editor.selected_document == (), "scene teardown retires the editor target"),
    ], "UI draft retirement", "UI_REVIEW");
}
fn ui_save_rejected(target) {
    let result = cmd("SaveScenario", #{target: target});
    report_verdict([
        t_true(!result.ok, "applying source to an unscripted target rejects terminally"),
        t_true(query("GetReadiness").application_available, "rejected source authoring preserves the application"),
    ], "UI source rejection", "UI_REVIEW");
}
fn ui_valid_draft() {
    let editor = query("InspectBehaviourEditor");
    report_verdict([t_true(!editor.dirty && !editor.pending,
        "successful compile consumes the exact submitted draft")], "UI valid draft", "UI_REVIEW");
}
fn ui_document_applied() {
    let modified = false;
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.dirty { modified = true; }
    }
    report_verdict([t_true(modified, "applying source modifies the document before disk save")], "UI document apply", "UI_REVIEW");
}
fn ui_source_read_only(doc) {
    let source = query("InspectScriptDocument", #{doc_id: doc}).source;
    report_verdict([t_true(!source.contains("FORBIDDEN_EDIT"), "Source Viewer rejects text mutation")], "UI read only source", "UI_REVIEW");
}

fn ui_intent_keys(intent, expected) {
    let actual = ();
    for row in query("InspectInputBindings").global_intents {
        if row.name == intent { actual = row.keys; }
    }
    report_verdict([t_true(actual == expected, "native keybinding edit reaches the shared persisted owner")], "UI binding " + intent, "UI_REVIEW");
}
fn ui_camera_empty_state() {
    let status = query("ReadExposures").surfaces["camera-status"];
    report_verdict([t_true(status.properties.active_label == "No active camera", "no camera has an explicit human-readable state")], "UI empty camera", "UI_REVIEW");
}

fn ui_export_rejected() {
    let result = cmd("ExportText", #{suggested_name:"", content:"", path:""});
    report_verdict([
        t_true(!result.ok, "an invalid export rejects before opening a destination picker"),
        t_true(query("GetReadiness").application_available, "export rejection preserves the workbench"),
    ], "UI export rejection", "UI_REVIEW");
}

fn ui_sysml_source_open() {
    let found = false;
    for document in query("ListOpenDocuments").open_documents {
        if document.kind == "sysml" { found = true; }
    }
    report_verdict([t_true(found, "requirement source action opens its stable indexed document")], "UI requirement source", "UI_REVIEW");
}

fn ui_sysml_large_inventory() {
    let analyzed = query("AnalyzeSysml", #{path: "twin://" + twin_source_identity(), tables: ["requirements"]});
    let usages = 0;
    for requirement in analyzed.analysis.requirements {
        if requirement.element.kind == "RequirementUsage" { usages += 1; }
    }
    report_verdict([t_true(usages == 100, "large scrolling source set has all 100 requirement usages")], "UI large requirements", "UI_REVIEW");
}
