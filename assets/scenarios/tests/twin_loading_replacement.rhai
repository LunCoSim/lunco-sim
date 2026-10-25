// Windowed API driver replaces a Twin while its scene is still loading.
// Keep the retirement verdict in Rhai; screenshots prove menu presentation.
fn verify_loading_replacement(retired_ids, replacement_root, retired_root) {
    let checks = [];
    let twin = query("ListTwin", #{ limit: 0 });
    checks.push(t_true(twin != () && twin.open == true && twin.root == replacement_root,
        "the replacement Twin owns the workspace"));
    let docs = query("ListOpenDocuments");
    checks.push(t_true(docs != () && !docs.open_documents.is_empty(),
        "the replacement scene has an open document"));
    checks.push(t_true(!retired_ids.is_empty(), "the canceled load has an outgoing document"));
    for doc in docs.open_documents {
        checks.push(t_true(doc.runtime_context.kind == "local_twin",
            "session restoration cannot detach a replacement document from its Twin owner"));
        let retired = false;
        for id in retired_ids { if id.to_string() == doc.doc_id.to_string() { retired = true; } }
        checks.push(t_true(!retired,
            "a canceled scene document cannot survive replacement"));
    }
    let outgoing = query("QueryUsdPrim", #{ path: retired_root });
    checks.push(t_true(outgoing != () && outgoing.ok == false && outgoing.code == 404 && find_path(retired_root) < 0,
        "the canceled scene cannot publish its root"));
    let current = query("QueryUsdPrim", #{ path: "/World" });
    checks.push(t_true(current != () && current.path == "/World" && current.spawned == true,
        "the replacement scene root is projected"));
    let physics = query("PhysicsPerformance");
    checks.push(t_true(physics.ok != false && physics.bodies == 0 && physics.colliders == 0
        && physics.joints == 0 && physics.joint_graph_edges == 0 && physics.pending_joints == 0,
        "the empty replacement owns no outgoing native or pending physics topology: " + physics.to_string()));
    report_verdict(checks, "Twin replacement during loading", "TWIN_LOADING_REPLACEMENT")
}

fn verify_rejected_loading_replacement(retained_ids) {
    let checks = [];
    let docs = query("ListOpenDocuments");
    for id in retained_ids {
        let present = false;
        for doc in docs.open_documents {
            if doc.doc_id.to_string() == id.to_string() { present = true; }
        }
        checks.push(t_true(present, "an invalid candidate preserves the current document"));
    }
    checks.push(t_true(!retained_ids.is_empty(), "the rejection probe has a live document"));
    report_verdict(checks, "Rejected loading replacement", "TWIN_LOADING_REPLACEMENT_NEGATIVE")
}

fn verify_cleared_loading(twin_root, retired_root) {
    let twin = query("ListTwin", #{limit: 0});
    let scene = query("QueryUsdPrim", #{path: retired_root});
    let readiness = query("GetReadiness", #{});
    let physics = query("PhysicsPerformance", #{});
    report_verdict([
        t_true(twin.open == true && twin.root == twin_root, "scene clear preserves the open Twin owner"),
        t_true(scene.ok == false && scene.code == 500
            && scene.error.contains("exactly one mounted live stage is required")
            && find_path(retired_root) < 0 && find_path("/World") < 0,
            "cleared scene has no running stage or prior projected root"),
        t_true(physics.ok != false && physics.bodies.to_string() == "0"
            && physics.colliders.to_string() == "0" && physics.joints.to_string() == "0",
            "cleared scene has no retained physics topology: " + physics.to_string()),
        t_true(readiness.ready == true && readiness.world_hold == false && readiness.faulted == false,
            "cleared loading scene leaves no pending preparation or fault")
    ], "Clear during loading", "TWIN_LOADING_CLEAR")
}

fn verify_startup_replacement(replacement_root) {
    let twin = query("ListTwin", #{limit: 0});
    let ready = query("GetReadiness", #{});
    let diagnostics = query("RuntimeDiagnostics", #{});
    let scene = query("QueryUsdPrim", #{path: "/World"});
    report_verdict([
        t_true(twin.open == true && twin.root == replacement_root,
            "the replacement owns the workspace after startup cancellation"),
        t_true(ready.ready == true && ready.world_hold == false && ready.faulted == false,
            "retired startup work cannot terminate or hold the replacement"),
        t_true(diagnostics.errors == 0, "replacement diagnostics are clear"),
        t_true(scene.spawned == true && find_path("/World") >= 0,
            "the replacement projects its scene"),
    ], "Startup Twin replacement", "STARTUP_TWIN_REPLACEMENT")
}
