// Runtime verdicts for the scene-loading scopes and selected scene.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["GetSceneLoadingSettings"], required_inputs: [] }
}

fn runner_ready(me) {
    let setting = query("GetSceneLoadingSettings", #{});
    setting != () && setting.scene_override == "gate.usda" && find_path("/World/RunnerMarker") >= 0
}

fn runner_verdict(me) {
    let setting = query("GetSceneLoadingSettings", #{});
    report_verdict([
        t_true(setting.scene_override == "gate.usda", "scene test keeps its explicit startup scene"),
        t_true(setting.admitted == false && setting.admitted_source == "twin",
            "explicit test scene mounts while the Twin disables its default"),
        t_true(find_path("/World/RunnerMarker") >= 0, "the selected test scene is projected")
    ], "SCENE RUNNER OVERRIDE", "SCENE_RUNNER_OVERRIDE")
}

fn task(me, ctx) {
    seq([wait_until(Fn("runner_ready")), once(Fn("runner_verdict"))])
}
fn verify_scene_loading(expected, source, requested, marker) {
    let setting = query("GetSceneLoadingSettings", #{});
    let readiness = query("GetReadiness", #{});
    let default_present = find_path("/World/DefaultMarker") >= 0;
    let explicit_present = find_path("/World/ExplicitMarker") >= 0;
    let tool_loaded = false;
    for library in query("ListToolLibraries", #{}).libraries {
        if library.name == "scene_loading_gate" { tool_loaded = true; }
    }
    let checks = [
        t_true(setting != () && setting.effective == expected,
            "effective scene-loading value matches the scoped settings"),
        t_true(setting.admitted == expected && setting.admitted_source == source,
            "the Twin admitted the expected setting source"),
        t_true(setting.scene_override == requested,
            "the runtime scene override is separate from the Twin manifest"),
        t_true(default_present == (marker == "default") && explicit_present == (marker == "explicit"),
            "the selected scene, or empty viewport, is projected"),
        t_true(tool_loaded, "Twin tool libraries load even with an empty scene"),
        t_true(readiness.ready && !readiness.world_hold && !readiness.faulted,
            "scene selection completes without leaving the world held or faulted")
    ];
    report_verdict(checks, "SCENE LOADING SETTINGS", "SCENE_LOADING_SETTINGS")
}

fn verify_retained_twin(expected_id) {
    let twin = query("ReadActiveTwinContract", #{});
    let readiness = query("GetReadiness", #{});
    report_verdict([
        t_true(twin.open && twin.twin_id == expected_id, "rejected project settings retain the active Twin"),
        t_true(readiness.ready && !readiness.world_hold && !readiness.faulted,
            "invalid project settings do not crash or hold the active session")
    ], "INVALID PROJECT SETTINGS", "INVALID_PROJECT_SETTINGS")
}

fn verify_ephemeral_workspace() {
    let workspace = query("ReadActiveWorkspace", #{});
    let twin = query("ReadActiveTwinContract", #{});
    let settings = query("GetSceneLoadingSettings", #{});
    let loaded = false;
    for library in query("ListToolLibraries", #{}).libraries {
        if library.name == "scene_loading_gate" { loaded = true; }
    }
    report_verdict([
        t_true(workspace.open && workspace.kind == "ephemeral", "the folder is an ephemeral workspace"),
        t_true(!twin.open, "no authored Twin is open"),
        t_true(!loaded, "Twin tool libraries are not activated in a temporary workspace"),
        t_true(settings.scene_override == "plain.usda" && find_path("/World/ExplicitMarker") >= 0,
            "an explicitly selected scene loads in the temporary workspace")
    ], "EPHEMERAL WORKSPACE", "EPHEMERAL_WORKSPACE")
}

fn verify_project_write_failure() {
    let setting = query("GetSceneLoadingSettings", #{});
    report_verdict([
        t_true(setting.project_error != () && setting.project_error.contains("could not deserialize"),
            "a malformed project settings document reports the write failure"),
        t_true(!setting.project_pending && setting.project == true,
            "the failed write finishes and preserves the previously admitted project value")
    ], "PROJECT SETTINGS WRITE FAILURE", "PROJECT_SETTINGS_WRITE_FAILURE")
}

fn verify_settings_menu(project, global, source) {
    let setting = query("GetSceneLoadingSettings", #{});
    report_verdict([
        t_true(setting.global == global, "the global setting matches the menu selection"),
        t_true(setting.project == project, "the project override matches the menu selection"),
        t_true(!setting.effective && setting.source == source, "the next-open value uses the expected scope")
    ], "SCENE SETTINGS MENU", "SCENE_SETTINGS_MENU")
}

fn verify_empty_scene_editor(doc, path) {
    let setting = query("GetSceneLoadingSettings", #{});
    let workbench = query("InspectWorkbench", #{});
    let document = query("InspectUsdDocument", #{doc_id: doc, path: path});
    let readiness = query("GetReadiness", #{});
    let preview_ready = false;
    for preview in query("InspectUsdViewport", #{}).previews {
        if preview.doc_id == doc && preview.projection_ready && preview.edit_target == "@root@"
            && preview.projected_generation == document.generation {
            preview_ready = true;
        }
    }
    report_verdict([
        t_true(!setting.admitted && setting.scene_override == (), "automatic scene loading remains disabled"),
        t_true(workbench.active_perspective == "editor", "the Editor perspective is active"),
        t_true(document.prim.exists, "the requested USD document contains the expected prim"),
        t_true(preview_ready, "the document has a ready Editor preview and root edit target"),
        t_true(readiness.ready && !readiness.world_hold && !readiness.faulted, "the empty-scene session remains ready")
    ], "EMPTY SCENE EDITOR", "EMPTY_SCENE_EDITOR")
}
