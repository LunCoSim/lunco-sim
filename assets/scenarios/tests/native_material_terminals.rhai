// Production command-clock gate for native UsdShade terminal resolution.
fn material_root() { "/World/NativeMaterialTerminal" }
fn material_shape() { material_root() + "/Shape" }
fn material_shader() { material_root() + "/Mat/Preview" }
fn material_graph() { material_root() + "/Mat/Graph" }
fn material_entity(doc) {
    for document in query("ListOpenDocuments").open_documents {
        if document.kind == "usd" && document.doc_id.to_string() == doc.to_string() {
            return find_path_in_document(document.doc_id, material_shape());
        }
    }
    throw("material fixture document is not open");
}
fn material_add(parent, name, kind) {
    #{AddPrim: #{edit_target: "@root@", parent_path: parent, name: name,
        type_name: kind, reference: (), reference_prim_path: ()}}
}
fn material_attr(path, name, kind, value) {
    #{SetAttribute: #{edit_target: "@root@", path: path, name: name, type_name: kind, value: value}}
}
fn material_connect(path, source) {
    #{SetConnection: #{edit_target: "@root@", path: path, name: "outputs:surface",
        type_name: "token", sources: [source + ".outputs:surface"]}}
}
fn material_prepare(doc) {
    let root = material_root(); let mat = root + "/Mat";
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Native material graph", ops: [
        material_add("/World", "NativeMaterialTerminal", "Scope"),
        material_add(root, "Shape", "Cube"),
        #{SetApiSchemas: #{edit_target: "@root@", path: material_shape(), schemas: ["MaterialBindingAPI"]}},
        material_add(root, "Mat", "Material"),
        material_add(mat, "Preview", "Shader"),
        material_attr(material_shader(), "info:id", "token", "\"UsdPreviewSurface\""),
        material_attr(material_shader(), "inputs:diffuseColor", "color3f", "(0.2,0.4,0.6)"),
        material_attr(material_shader(), "inputs:roughness", "float", "0.3"),
        material_attr(material_shader(), "outputs:surface", "token", "\"surface\""),
        material_add(mat, "Graph", "NodeGraph"),
        material_connect(material_graph(), material_shader()),
        material_connect(mat, material_graph()),
        #{SetRelationship: #{edit_target: "@root@", path: material_shape(),
            name: "material:binding", targets: [mat]}}
    ]});
    if result.ok != true || result.status != "applied" { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn material_valid(doc) {
    let shape = material_entity(doc);
    if shape < 0 { print("MATERIAL_PENDING"); return; }
    let look = get(shape, "PbrLook");
    if get(shape, "Mesh3d") == () || look == () { print("MATERIAL_PENDING"); return; }
    let topology = query("QueryUsdPrim", #{doc_id: doc, path: material_shape(), topology: true}).topology;
    report_verdict([
        t_true(topology.parts[0].materials.shader == material_shader(),
            "inspection resolves the native Shader through the NodeGraph"),
        t_true(abs(look.base_color.red - 0.2) < 0.001 && abs(look.base_color.green - 0.4) < 0.001 &&
            abs(look.base_color.blue - 0.6) < 0.001, "PBR consumer uses the terminal shader color"),
        t_true(abs(look.perceptual_roughness - 0.3) < 0.001,
            "PBR consumer uses the terminal shader roughness")
    ], "NATIVE MATERIAL TERMINAL", "NATIVE_MATERIAL_TERMINAL");
}
fn material_edit(doc) {
    let result = cmd("SetObjectProperty", #{entity_id: material_entity(doc), property: "roughness", value: "0.7"});
    if result.ok != true || result.status != "applied" { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn material_edited(doc) {
    let shape = material_entity(doc);
    let look = get(shape, "PbrLook");
    if abs(look.perceptual_roughness - 0.7) >= 0.001 { print("MATERIAL_PENDING"); return; }
    let shader = query("QueryUsdPrim", #{doc_id: doc, path: material_shader(), attrs: ["inputs:roughness"]});
    let graph = query("QueryUsdPrim", #{doc_id: doc, path: material_graph(), attrs: ["inputs:roughness"]});
    if shader.ok == false || graph.ok == false {
        if shader.to_string().contains("projection is not current") || graph.to_string().contains("projection is not current") {
            print("MATERIAL_PENDING"); return;
        }
        throw(shader.to_string() + " GRAPH " + graph.to_string());
    }
    if shader == () || graph == () { print("MATERIAL_PENDING"); return; }
    report_verdict([
        t_true(abs(shader.attrs["inputs:roughness"] - 0.7) < 0.001, "material edit authors the producing Shader input"),
        t_true(graph.attrs["inputs:roughness"] == (), "material edit leaves NodeGraph inputs untouched"),
        t_true(abs(look.perceptual_roughness - 0.7) < 0.001, "terminal edit reaches the PBR consumer")
    ], "NATIVE MATERIAL EDIT", "NATIVE_MATERIAL_EDIT");
}
fn material_cycle(doc) {
    let result = cmd("ApplyUsdOp", #{doc_id: doc, parent_gen: (),
        op: material_connect(material_graph(), material_root() + "/Mat")});
    if result.ok != true || result.status != "applied" { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn material_invalid(doc) {
    let shape = material_entity(doc);
    if shape < 0 || get(shape, "UsdSceneProjectionFailed") == () { print("MATERIAL_PENDING"); return; }
    let inspection = query("QueryUsdPrim", #{doc_id: doc, path: material_shape(), topology: true});
    let rejected = inspection.ok == false && inspection.to_string().contains("no producing shader");
    let before = get(shape, "PbrLook").perceptual_roughness;
    let edit = cmd("SetObjectProperty", #{entity_id: shape, property: "roughness", value: "0.1"});
    report_verdict([
        t_true(get(shape, "UsdSceneProjectionFailed").to_string().contains("no producing shader"),
            "unresolved surface cycle is a projection diagnostic"),
        t_true(get(shape, "Visibility").to_string().contains("Hidden"),
            "unresolved surface cycle hides the affected projection"),
        t_true(rejected,
            "inspection preserves the terminal failure instead of returning an absent shader"),
        t_true(edit.ok == false && edit.status == "rejected" && edit.to_string().contains("no producing shader"),
            "material edit returns a terminal native graph rejection"),
        t_true(get(shape, "PbrLook").perceptual_roughness == before,
            "rejected material edit preserves the live PBR intent")
    ], "NATIVE MATERIAL TERMINAL NEGATIVE", "NATIVE_MATERIAL_TERMINAL_NEGATIVE");
}
