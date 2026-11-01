// Production command-clock gate for initial, bounded hierarchy projection.
fn admission_leaf() {
    let path = "/World/AdmissionChain";
    for index in 1..64 { path += "/Child"; }
    path
}
fn admission_verify(doc, started) {
    if find(admission_leaf()) < 0 {
        print("ADMISSION_PENDING"); return;
    }
    let prim = query("QueryUsdPrim", #{doc_id: doc, path: admission_leaf()});
    let elapsed = query("EnginePerformance").frame_sample_count.to_int() - started;
    print("ADMISSION_UPDATES " + elapsed);
    let complete = true;
    let path = "/World/AdmissionChain";
    for index in 0..64 {
        if find(path) < 0 { complete = false; }
        path += "/Child";
    }
    for index in 0..1024 {
        if find("/World/AdmissionChain/Batch" + index) < 0 { complete = false; }
    }
    report_verdict([
        t_true(elapsed <= 32, "64 levels and 1024 siblings project within 32 updates including document admission: " + elapsed),
        t_true(complete, "all admitted ancestors and siblings remain projected"),
        t_true(prim.type_name == "Scope", "saved fixture leaf reads back through its document owner")
    ], "USD CHILD ADMISSION", "USD_CHILD_ADMISSION");
}
fn admission_invalid(doc) {
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (),
        label: "Reject malformed child", ops: [#{AddPrim: #{edit_target: "@runtime@",
            parent_path: "/World", name: "Bad/Child", type_name: "Scope",
            reference: (), reference_prim_path: ()}}]});
    report_verdict([
        t_true(result.ok == false && result.status != "pending" && result.error != (),
            "malformed child name is terminally rejected"),
        t_true(find("/World/Bad") < 0,
            "rejected child admission creates no partial parent"),
        t_true(find(admission_leaf()) >= 0,
            "rejected authoring preserves the projected hierarchy")
    ], "USD CHILD ADMISSION NEGATIVE", "USD_CHILD_ADMISSION_NEGATIVE");
}

fn admission_pending() {
    print(find("/World/AdmissionChain") >= 0 && find(admission_leaf()) < 0);
}

fn admission_retired() {
    report_verdict([
        t_true(find("/World/AdmissionChain") < 0, "retired parent is absent"),
        t_true(find(admission_leaf()) < 0, "retired continuation cannot publish a late child"),
        t_true(find("/World/TestLight") >= 0, "replacement scene remains projected")
    ], "USD CHILD ADMISSION RETIRED", "USD_CHILD_ADMISSION_RETIRED");
}
