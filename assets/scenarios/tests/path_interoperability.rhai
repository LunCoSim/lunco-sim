// One-shot production API driver assertions. The driver owns a temporary Twin
// and supplies its exact document IDs, filenames and rejection diagnostics.
fn normalized_path(path) {
    path.replace("\\", "/");
    path
}

fn document_record(id) {
    let docs = query("ListOpenDocuments", #{});
    for doc in docs.open_documents { if doc.doc_id.to_string() == id.to_string() { return doc; } }
    throw "path probe document is missing";
}

fn verify_model_path(id, filename, equation, saved) {
    let record = document_record(id);
    let source = query("GetDocumentSource", #{doc_id: record.doc_id});
    let compile = query("CompileStatus", #{doc_id: record.doc_id});
    report_verdict([
        t_true(normalized_path(record.origin.path).ends_with("/" + filename), "workspace origin follows the renamed filename"),
        t_true(normalized_path(source.origin.path).ends_with("/" + filename), "Modelica source origin follows the renamed filename"),
        t_true(source.source.contains(equation), "current model source has the expected equation"),
        t_true(!saved || source.dirty == false, "saved model source is clean"),
        t_true(compile.ast_parsed == true && compile.candidates.contains("PortableProbe"), "model parser retained the authored class"),
    ], "Portable model document", "PATH_MODEL_DOCUMENT")
}

fn verify_rename_rejection(id, filename, diagnostic) {
    let record = document_record(id);
    report_verdict([
        t_true(diagnostic != "", "invalid rename returned a terminal error"),
        t_true(normalized_path(record.origin.path).ends_with("/" + filename), "rejected rename preserved the open document"),
    ], "Rejected portable rename", "PATH_RENAME_REJECTED")
}

fn verify_saved_tree(old_names, current_name) {
    let twin = query("ListTwin", #{});
    let paths = [];
    for file in twin.files { paths.push(normalized_path(file.relative_path)); }
    let checks = [t_true(paths.contains(current_name), "saved model is indexed at its current name")];
    for name in old_names { checks.push(t_true(!paths.contains(name), "save did not recreate " + name)); }
    report_verdict(checks, "Portable model persistence", "PATH_MODEL_PERSISTENCE")
}

fn literal_model_ready() {
    let entity = find_path("/World/LiteralModel");
    if entity < 0 { return false; }
    let ports = query("ReadPorts", #{api_id: entity, port_names: ["x"]});
    if ports == () || ports.ports == () { return false; }
    for port in ports.ports {
        if port.name == "x" && port.direction == "out" && port.value != () {
            return (port.value - 2.0).abs() < 0.000001;
        }
    }
    false
}

fn verify_usd_path(id, filename) {
    let record = document_record(id);
    let document = query("InspectUsdDocument", #{doc_id: record.doc_id, path: "/World/LiteralProbe"});
    let target = query("ResolveUsdTarget", #{doc_id: record.doc_id, path: "/World/LiteralProbe", edit_target: "@root@"});
    print("USD path probe target: " + target);
    report_verdict([
        t_true(normalized_path(record.origin.path).ends_with("/" + filename), "USD document retained its literal filename"),
        t_true(document.prim.exists && document.prim.type == "Cube", "saved USD prim was read back"),
        t_true(target.status == "resolved" && target.source == "canonical_stage", "literal USD filename resolves to the loaded canonical stage"),
        t_true(literal_model_ready(), "literal Modelica source compiled and published its authored equation result"),
    ], "Portable USD and Modelica runtime", "PATH_USD_MODELICA")
}
