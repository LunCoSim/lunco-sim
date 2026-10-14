// Production command-clock gate for native subtree activation.
fn activity_root() { "/World/NativeActivity" }
fn activity_child() { "/World/NativeActivity/Child" }
fn activity_prepare(doc) {
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Native activity fixture", ops: [
        #{AddPrim: #{edit_target: "@runtime@", parent_path: "/World", name: "NativeActivity",
            type_name: "Scope", reference: (), reference_prim_path: ()}},
        #{AddPrim: #{edit_target: "@runtime@", parent_path: activity_root(), name: "Child",
            type_name: "Cube", reference: (), reference_prim_path: ()}}
    ]});
    if result.ok != true || result.status != "applied" { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn activity_enabled(doc) {
    let child = find(activity_child());
    if child < 0 { print("ACTIVITY_PENDING"); return; }
    if get(child, "Mesh3d") == () { print("ACTIVITY_PENDING"); return; }
    let prim = query("QueryUsdPrim", #{doc_id: doc, path: activity_child()});
    report_verdict([
        t_true(prim.ok != false && prim.active == true, "native active child projects its mesh"),
        t_true(query("GetReadiness").faulted == false, "active subtree admission remains non-faulted")
    ], "NATIVE ACTIVITY", "NATIVE_ACTIVITY");
}
fn activity_set(doc, active) {
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Native subtree activity", ops: [
        #{SetActive: #{edit_target: "@runtime@", path: activity_root(), active: active}}
    ]});
    if result.ok != true || result.status != "applied" { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn activity_disable(doc) { activity_set(doc, false); }
fn activity_enable(doc) { activity_set(doc, true); }
fn activity_disabled(doc) {
    if find(activity_root()) >= 0 || find(activity_child()) >= 0 { print("ACTIVITY_PENDING"); return; }
    report_verdict([
        t_true(find(activity_root()) < 0, "inactive root has no runtime projection"),
        t_true(find(activity_child()) < 0, "inactive ancestor prunes the child projection"),
        t_true(query("GetReadiness").faulted == false, "valid deactivation remains non-faulted")
    ], "NATIVE ACTIVITY DISABLED", "NATIVE_ACTIVITY_DISABLED");
}
fn activity_invalid(doc) {
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Reject malformed activation", ops: [
        #{SetActive: #{edit_target: "@runtime@", path: activity_root(), active: "invalid"}}
    ]});
    report_verdict([
        t_true(result.ok == false && result.status != "pending" && result.error != (),
            "malformed activation is a terminal command diagnostic"),
        t_true(find(activity_child()) < 0, "malformed activation cannot reactivate the child")
    ], "NATIVE ACTIVITY NEGATIVE", "NATIVE_ACTIVITY_NEGATIVE");
}
