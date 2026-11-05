// Called through the production REPL while a referenced USD scene is admitted.
fn install_atomic_surface_policy() {
    cmd("SetRhaiPolicy", #{name: "AtomicSurface", seam: "runtime.ui.surface", entry: "project",
        source: `fn project(facts) { #{visible:true, properties:#{atomic_marker:"complete"}} }`,
        deterministic: false})
}
fn atomic_surface_policy_ready() {
    let result = invoke_hook("runtime.ui.surface", [#{}]);
    result.ok && result.value.properties.atomic_marker == "complete"
}
fn verify_atomic_surface_policy(doc, path) {
    let document = query("InspectUsdDocument", #{doc_id:doc, path:path});
    let prim = query("QueryUsdPrim", #{doc_id:doc, path:path,
        attrs:["lunco:policy:seam", "lunco:policy:entry", "info:sourceCode", "lunco:policy:deterministic"]});
    let camera = query("ReadExposures", #{surface:"camera-status"}).surfaces["camera-status"];
    report_verdict([
        t_true(document.prim.type == "LunCoPolicy", "the complete policy has its registered USD type"),
        t_true(prim.attrs["lunco:policy:seam"] == "runtime.ui.surface"
            && prim.attrs["lunco:policy:entry"] == "project", "seam and entry commit together"),
        t_true(prim.attrs["info:sourceCode"].contains("atomic_marker")
            && prim.attrs["lunco:policy:deterministic"] == false, "source and execution contract commit together"),
        t_true(atomic_surface_policy_ready(), "the complete USD policy installs through its production owner"),
        t_true(camera.state == "ready" && camera.visible
            && camera.properties.atomic_marker == "complete", "the real surface consumer observes the complete policy"),
        t_true(!query("GetReadiness").faulted, "policy installation does not fault the simulation")
    ], "Atomic surface policy", "ATOMIC_SURFACE_POLICY");
}
fn atomic_surface_policy_removed(doc, path) {
    !query("InspectUsdDocument", #{doc_id:doc, path:path}).prim.exists
}
fn verify_atomic_surface_policy_undo(doc, path) {
    let scope = path.sub_string(0, path.len() - "/AtomicSurface".len());
    report_verdict([
        t_true(atomic_surface_policy_removed(doc,path), "one undo removes the whole policy"),
        t_true(!query("InspectUsdDocument", #{doc_id:doc,path:scope}).prim.exists,
            "one undo also removes the scope created by that intent"),
        t_true(!atomic_surface_policy_ready(), "undo retires the policy implementation"),
        t_true(!query("GetReadiness").faulted, "policy undo preserves the running world")
    ], "Atomic surface policy undo", "ATOMIC_SURFACE_POLICY_UNDO");
}
