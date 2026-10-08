// Windowed API driver replaces a Twin while its scene is still loading.
// Keep the retirement verdict in Rhai; screenshots prove menu presentation.
fn verify_loading_replacement(retired_ids, replacement_root) {
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
    let outgoing = query("QueryUsdPrim", #{ path: "/SandboxScene" });
    checks.push(t_true(outgoing != () && outgoing.ok == false && outgoing.code == 404,
        "the canceled sandbox cannot publish its scene root"));
    let current = query("QueryUsdPrim", #{ path: "/World" });
    checks.push(t_true(current != () && current.path == "/World" && current.spawned == true,
        "the replacement scene root is projected"));
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
