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

fn verify_resource_edit(id, package_name, dirty) {
    let source = query("GetDocumentSource", #{doc_id: id});
    let status = query("CompileStatus", #{doc_id: id});
    report_verdict([
        t_true(source.source.contains("package " + package_name), "current document owns the edited declaration"),
        t_true(source.dirty == dirty, "document save state matches the current source"),
        t_true(status.ast_parsed, "current source generation has a valid parse"),
    ], "Resource source generation", "ASSET_RESOURCE_GENERATION");
}

fn verify_resource_invalid(id, expected_source) {
    let source = query("GetDocumentSource", #{doc_id: id});
    let status = query("CompileStatus", #{doc_id: id});
    report_verdict([
        t_true(source.source == expected_source, "invalid edit is the current authored source"),
        t_true(!status.ast_parsed, "invalid source is not admitted as a parsed generation"),
    ], "Resource invalid generation", "ASSET_RESOURCE_INVALID");
}

fn verify_resource_rename(id, expected_path, class_name) {
    let source = query("GetDocumentSource", #{doc_id: id});
    let status = query("CompileStatus", #{doc_id: id});
    report_verdict([
        t_true(source.origin.path == expected_path, "renamed document owns its current path"),
        t_true(source.source.contains("package " + class_name), "renamed source owns its current declaration"),
        t_true(!source.dirty, "rename preserves saved source"),
        t_true(status.ast_parsed, "renamed source has a current valid parse"),
    ], "Resource renamed source", "ASSET_RESOURCE_RENAME");
}
