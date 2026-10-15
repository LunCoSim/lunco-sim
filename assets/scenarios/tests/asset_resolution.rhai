// One-shot assertions for the owned production asset/UI driver.
fn verify_scheme_scope(lower_diagnostic, mixed_diagnostic) {
    report_verdict([
        t_true(lower_diagnostic.contains("without an active Twin"), "lowercase Twin source requires its owner"),
        t_true(mixed_diagnostic.contains("without an active Twin"), "mixed-case scheme requires the same owner"),
    ], "Asset scheme scope", "ASSET_SCHEME_SCOPE");
}

fn verify_resource_source(id, expected_open) {
    let found = false;
    for document in query("ListOpenDocuments", #{}).open_documents {
        if document.doc_id.to_string() == id.to_string() { found = true; }
    }
    report_verdict([
        t_true(found == expected_open, "resource source document has the expected lifetime"),
    ], "Resource source lifetime", "ASSET_RESOURCE_SOURCE");
}
