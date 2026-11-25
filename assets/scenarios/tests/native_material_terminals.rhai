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
    let alternate = root + "/Alternate"; let alternate_shader = alternate + "/Preview";
    let result = cmd("ApplyUsdOps", #{doc_id: doc, parent_gen: (), label: "Native material graph", ops: [
        material_add("/World", "NativeMaterialTerminal", "Scope"),
        material_add(root, "Shape", "Cube"),
        material_add(root, "Unrelated", "Cube"),
        material_add(root, "Alternate", "Material"),
        material_add(alternate, "Preview", "Shader"),
        material_attr(alternate_shader, "info:id", "token", "\"UsdPreviewSurface\""),
        material_attr(alternate_shader, "inputs:diffuseColor", "color3f", "(0.8,0.2,0.1)"),
        material_attr(alternate_shader, "outputs:surface", "token", "\"surface\""),
        material_connect(alternate, alternate_shader),
        #{SetApiSchemas: #{edit_target: "@root@", path: material_shape(), schemas: ["MaterialBindingAPI"]}},
        material_add(root, "Mat", "Material"),
        material_add(mat, "InvalidImage", "Shader"),
        material_attr(mat+"/InvalidImage", "info:id", "token", "\"UsdUVTexture\""),
        material_attr(mat+"/InvalidImage", "inputs:file", "asset", "@lunco://shaders/pbr_lit.wesl@"),
        material_attr(mat+"/InvalidImage", "outputs:rgb", "float3", "(0,0,0)"),
        material_add(mat, "Preview", "Shader"),
        material_attr(material_shader(), "info:id", "token", "\"UsdPreviewSurface\""),
        material_attr(material_shader(), "inputs:diffuseColor", "color3f", "(0.2,0.4,0.6)"),
        material_attr(material_shader(), "inputs:roughness", "float", "0.3"),
        material_attr(material_shader(), "outputs:surface", "token", "\"surface\""),
        material_add(mat, "Custom", "Shader"),
        material_attr(mat+"/Custom", "info:implementationSource", "token", "\"sourceAsset\""),
        material_attr(mat+"/Custom", "info:wesl:sourceAsset", "asset", "@lunco://shaders/pbr_lit.wesl@"),
        material_attr(mat+"/Custom", "outputs:surface", "token", "\"surface\""),
        material_add(mat, "Second", "Shader"),
        material_attr(mat+"/Second", "info:id", "token", "\"UsdPreviewSurface\""),
        material_attr(mat+"/Second", "inputs:diffuseColor", "color3f", "(0.1,0.9,0.1)"),
        material_attr(mat+"/Second", "outputs:surface", "token", "\"surface\""),
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
    let inspected=query("QueryUsdPrim",#{doc_id:doc,path:material_shape(),topology:true});
    if inspected==() || inspected.topology==() {print("MATERIAL_PENDING");return;}
    let topology=inspected.topology;
    if abs(look.base_color.red-0.2)>0.001 {print("MATERIAL_PENDING");return;}
    report_verdict([
        t_true(topology.parts[0].materials.shader == material_shader(),
            "inspection resolves the native Shader through the NodeGraph"),
        t_true(abs(look.base_color.red - 0.2) < 0.001 && abs(look.base_color.green - 0.4) < 0.001 &&
            abs(look.base_color.blue - 0.6) < 0.001, "PBR consumer uses the terminal shader color"),
        t_true(abs(look.perceptual_roughness - 0.3) < 0.001,
            "PBR consumer uses the terminal shader roughness")
    ], "NATIVE MATERIAL TERMINAL", "NATIVE_MATERIAL_TERMINAL");
}
fn material_binding_change(doc, alternate) {
    let shape=material_entity(doc);
    let unrelated=find_path(material_root()+"/Unrelated");
    if !set(shape,"Transform.translation.x",3.0) || !set(unrelated,"Transform.translation.y",5.0) {
        throw("runtime state probes could not be set");
    }
    let destination=material_root()+(if alternate {"/Alternate"} else {"/Mat"});
    let result=cmd("ApplyUsdOp",#{doc_id:doc,parent_gen:(),
        op:#{SetRelationship:#{edit_target:"@root@",path:material_shape(),name:"material:binding",targets:[destination]}}});
    if result.ok!=true || result.status!="applied" {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_rebind(doc) {material_binding_change(doc,true);}
fn material_restore(doc) {material_binding_change(doc,false);}
fn material_rebound(doc) {
    let shape=material_entity(doc);
    let look=get(shape,"PbrLook");
    if look==() || abs(look.base_color.red-0.8)>0.001 {print("MATERIAL_PENDING");return;}
    let unrelated=find_path(material_root()+"/Unrelated");
    report_verdict([
        t_true(abs(look.base_color.green-0.2)<0.001 && abs(look.base_color.blue-0.1)<0.001,
            "rebound geometry uses the new material"),
        t_true(get(shape,"Transform.translation.x")==3.0 && get(unrelated,"Transform.translation.y")==5.0,
            "material binding edits preserve runtime state of affected and unrelated geometry"),
        t_true(get(shape,"Mesh3d")!=() && get(unrelated,"Mesh3d")!=(),
            "material binding edits retain prepared geometry")
    ],"NATIVE MATERIAL REBIND","NATIVE_MATERIAL_REBIND");
}
fn material_annotate(doc) {
    let result = cmd("ApplyUsdOps", #{doc_id:doc, label:"Annotate shading network", ops:[
        material_attr(material_root(), "review:note", "string", "Checked"),
        material_attr(material_root()+"/Alternate", "review:note", "string", "Checked"),
        material_attr(material_graph(), "review:note", "string", "Checked"),
        material_attr(material_shader(), "review:note", "string", "Checked"),
        #{CopyProperty:#{edit_target:"@root@",from_path:material_shader()+".review:note",to_path:material_root()+"/Alternate.review:copy"}}
    ]});
    if !result.ok {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_annotated(doc) {
    let mat = query("QueryUsdPrim", #{doc_id:doc,path:material_root()+"/Alternate",attrs:["review:note","review:copy"]});
    if mat.attrs == () || mat.attrs["review:copy"] != "Checked" {print("MATERIAL_PENDING");return;}
    report_verdict([
        t_true(get(material_entity(doc),"Transform.translation.x")==3.0 &&
            get(find_path(material_root()+"/Unrelated"),"Transform.translation.y")==5.0,
            "shading annotations preserve bound and unrelated live poses")
    ],"NATIVE MATERIAL ANNOTATION","NATIVE_MATERIAL_ANNOTATION");
}
fn material_switch_terminal(doc) {
    let result = cmd("ApplyUsdOp", #{doc_id:doc, op:material_connect(material_root()+"/Mat",material_root()+"/Mat/Second")});
    if !result.ok {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_terminal_switched(doc) {
    let shape = material_entity(doc);
    let look = get(shape,"PbrLook");
    if look == () || abs(look.base_color.green - 0.9) > 0.001 { print("MATERIAL_PENDING"); return; }
    report_verdict([t_true(get(shape,"Transform.translation.x")==3.0 &&
        get(find_path(material_root()+"/Unrelated"),"Transform.translation.y")==5.0,
        "a new producing shader changes only the bound appearance")],
        "NATIVE MATERIAL RECONNECT","NATIVE_MATERIAL_RECONNECT");
}
fn material_switch_custom(doc) {
    let result = cmd("ApplyUsdOp", #{doc_id:doc, op:material_connect(material_root()+"/Mat",material_root()+"/Mat/Custom")});
    if !result.ok {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_custom(doc) {
    let shape = material_entity(doc);
    let prim=query("QueryUsdPrim",#{doc_id:doc,path:material_shape(),topology:true});
    if prim.topology==() || get(shape,"PbrLook")!=() {print("MATERIAL_PENDING");return;}
    report_verdict([t_true(prim.topology.parts[0].materials.shader == material_root()+"/Mat/Custom" &&
        get(shape,"UsdSceneProjectionFailed")==() && get(shape,"Transform.translation.x")==3.0,
        "custom shader owns the existing surface and preserves its live transform")],
        "NATIVE MATERIAL CUSTOM","NATIVE_MATERIAL_CUSTOM");
}
fn material_repair_cycle(doc) {
    let result=cmd("ApplyUsdOp",#{doc_id:doc,op:material_connect(material_graph(),material_shader())});
    if !result.ok {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_restored_graph(doc) {
    let shape = material_entity(doc);
    if get(shape, "UsdSceneProjectionFailed") != () { print("MATERIAL_PENDING"); return; }
    let look = get(shape, "PbrLook");
    report_verdict([
        t_true(look != () && abs(look.perceptual_roughness - 0.7) < 0.001 &&
            !get(shape, "Visibility").to_string().contains("Hidden"),
            "repairing the forwarding graph restores its surface and visibility"),
        t_true(get(shape, "Transform.translation.x") == 3.0 &&
            get(find_path(material_root()+"/Unrelated"), "Transform.translation.y") == 5.0,
            "repair preserves the bound and unrelated live transforms")
    ], "NATIVE MATERIAL GRAPH REPAIR", "NATIVE_MATERIAL_GRAPH_REPAIR");
}
fn material_restore_terminal(doc) {
    let result = cmd("ApplyUsdOp", #{doc_id:doc, op:material_connect(material_root()+"/Mat",material_graph())});
    if !result.ok {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_roughness_write(doc, value) {
    let result=cmd("ApplyUsdOp",#{doc_id:doc,parent_gen:(),
        op:material_attr(material_shader(),"inputs:roughness","float",value)});
    if result.ok!=true || result.status!="applied" {throw(result.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_bad_input(doc) { material_roughness_write(doc,"1.5"); }
fn material_bad_input_verdict(doc) {
    let shape=material_entity(doc);
    let err=get(shape,"UsdSceneProjectionFailed");
    if err==() {print("MATERIAL_PENDING");return;}
    report_verdict([
        t_true(err.to_string().contains("inputs:roughness") && get(shape,"Visibility").to_string().contains("Hidden"),
            "a malformed live input is diagnosed and withheld"),
        t_true(get(shape,"Transform.translation.x")==3.0 && abs(get(shape,"PbrLook").perceptual_roughness-0.3)<0.001,
            "malformed material input retains admitted pose and previous material intent")
    ],"NATIVE MATERIAL BAD INPUT","NATIVE_MATERIAL_BAD_INPUT");
}
fn material_repair_input(doc) { material_roughness_write(doc,"0.3"); }
fn material_repaired_input(doc) {
    let shape=material_entity(doc);
    if get(shape,"UsdSceneProjectionFailed")!=() {print("MATERIAL_PENDING");return;}
    report_verdict([
        t_true(!get(shape,"Visibility").to_string().contains("Hidden") && get(shape,"Mesh3d")!=(),
            "valid replacement input restores the already admitted visual"),
        t_true(get(shape,"Transform.translation.x")==3.0 && get(find_path(material_root()+"/Unrelated"),"Transform.translation.y")==5.0,
            "material repair does not reset geometry or unrelated runtime state")
    ],"NATIVE MATERIAL INPUT REPAIR","NATIVE_MATERIAL_INPUT_REPAIR");
}
fn material_assign_wesl(doc) {
    let shape = material_entity(doc);
    let before = query("QueryUsdPrim", #{doc_id: doc, path: material_shader()}).document_generation;
    let invalid = cmd("SetObjectProperty", #{entity_id: shape, property: "shader",
        value: "lunco://shaders/invalid.png", intent: "Persistent"});
    let after = query("QueryUsdPrim", #{doc_id: doc, path: material_shader()}).document_generation;
    if invalid.ok != false || invalid.status != "rejected" || after != before || get(shape,"PbrLook") == () {
        throw("Rejected shader assignment changed the document or its bound appearance: " + invalid.to_string());
    }
    let result = cmd("SetObjectProperty", #{entity_id: shape, property: "shader",
        value: "lunco://shaders/pbr_lit.wesl", intent: "Persistent"});
    if !result.ok { throw(result.to_string()); }
    print("FIXTURE_APPLIED");
}
fn material_assigned_wesl(doc) {
    let shape = material_entity(doc);
    let shader = query("QueryUsdPrim", #{doc_id: doc, path: material_shader(),
        attrs: ["info:implementationSource", "info:wesl:sourceAsset"]});
    if shader.attrs == () || get(shape, "PbrLook") != () { print("MATERIAL_PENDING"); return; }
    report_verdict([
        t_true(shader.attrs["info:implementationSource"] == "sourceAsset" &&
            shader.attrs["info:wesl:sourceAsset"] == "lunco://shaders/pbr_lit.wesl",
            "assigning a PreviewSurface selects the WESL source and stores the public asset identity"),
        t_true(get(shape, "UsdSceneProjectionFailed") == () && get(shape, "Transform.translation.x") == 3.0,
            "shader assignment preserves the live pose and projects the assigned surface")
    ], "NATIVE SHADER ASSIGNMENT", "NATIVE_SHADER_ASSIGNMENT");
}
fn material_edit(doc) {
    let shape = material_entity(doc);
    let read = #{doc_id:doc,path:material_shader(),attrs:["inputs:roughness"]};
    let before = query("QueryUsdPrim", read);
    let live = cmd("SetObjectProperty", #{entity_id:shape,property:"roughness",value:"0.45"});
    let after = query("QueryUsdPrim", read);
    if live.ok != true || abs(get(shape,"PbrLook").perceptual_roughness - 0.45) > 0.001 ||
        before.document_generation != after.document_generation || before.attrs != after.attrs {
        throw("live roughness must update the surface and retain authored inputs and generation");
    }
    let result = cmd("SetObjectProperty", #{entity_id: shape, property: "roughness", value: "0.7", intent:"Persistent"});
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
    let before = get(shape, "PbrLook");
    let edit = cmd("SetObjectProperty", #{entity_id: shape, property: "roughness", value: "0.1", intent:"Persistent"});
    report_verdict([
        t_true(get(shape, "UsdSceneProjectionFailed").to_string().contains("no producing shader"),
            "unresolved surface cycle is a projection diagnostic"),
        t_true(get(shape, "Visibility").to_string().contains("Hidden"),
            "unresolved surface cycle hides the affected projection"),
        t_true(rejected,
            "inspection preserves the terminal failure instead of returning an absent shader"),
        t_true(edit.ok == false && edit.status == "rejected" && edit.to_string().contains("no producing shader"),
            "material edit returns a terminal native graph rejection"),
        t_true(get(shape, "PbrLook") == before,
            "rejected material edit leaves the projection intent unchanged")
    ], "NATIVE MATERIAL TERMINAL NEGATIVE", "NATIVE_MATERIAL_TERMINAL_NEGATIVE");
}

fn material_remove_producer(doc) {
    let edit=cmd("ApplyUsdOp",#{doc_id:doc,op:#{RemovePrim:#{edit_target:"@root@",path:material_shader()}}});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_undo_edit(doc) {
    let edit=cmd("UndoDocument",#{doc_id:doc});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_bind_parent(doc) {
    let root=material_root();
    let edit=cmd("ApplyUsdOps",#{doc_id:doc,label:"Set inherited material",ops:[
        #{SetApiSchemas:#{edit_target:"@root@",path:root,schemas:["MaterialBindingAPI"]}},
        #{SetRelationship:#{edit_target:"@root@",path:root,name:"material:binding",targets:[root+"/Alternate"]}}
    ]});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_inherited(doc) {
    let child=find_path(material_root()+"/Unrelated");
    let inherited=get(child,"PbrLook");
    if inherited==() || abs(inherited.base_color.red-0.8)>0.001 {print("MATERIAL_PENDING");return;}
    let shape=material_entity(doc);
    let before=query("QueryUsdPrim",#{doc_id:doc,path:material_shape()});
    let edit=cmd("SetObjectProperty",#{entity_id:shape,property:"wheel_radius",value:"2",intent:"Persistent"});
    let after=query("QueryUsdPrim",#{doc_id:doc,path:material_shape(),attrs:["physxVehicleWheel:radius"]});
    report_verdict([
        t_true(abs(get(shape,"PbrLook").base_color.red-0.2)<0.001,"the stronger direct binding still wins"),
        t_true(get(shape,"Transform.translation.x")==3.0 && get(child,"Transform.translation.y")==5.0,
            "changing an inherited surface preserves live poses"),
        t_true(!edit.ok && edit.status=="rejected" && before.document_generation==after.document_generation &&
            after.attrs["physxVehicleWheel:radius"]==(),"a wheel edit on geometry rejects without changing USD")
    ],"NATIVE MATERIAL INHERITANCE","NATIVE_MATERIAL_INHERITANCE");
}

fn material_physical_setup(doc) {
    let root=material_root(); let mat=root+"/Alternate"; let body=root+"/ContactSurface";
    let edit=cmd("ApplyUsdOps",#{doc_id:doc,label:"Prepare physical surface",ops:[
        material_add(root,"ContactSurface","Cube"),
        #{SetApiSchemas:#{edit_target:"@root@",path:body,schemas:["PhysicsCollisionAPI","MaterialBindingAPI"]}},
        #{SetApiSchemas:#{edit_target:"@root@",path:mat,schemas:["PhysicsMaterialAPI"]}},
        material_attr(mat,"physics:dynamicFriction","float","0.81"),
        #{SetRelationship:#{edit_target:"@root@",path:body,name:"material:binding",targets:[root+"/Mat"]}}
    ]});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_contact_visual(doc) {
    let body=find_path(material_root()+"/ContactSurface");
    if body<0 || query("QueryPhysicsState",#{id:body}).collider_present!=true {print("MATERIAL_PENDING");return;}
    let friction=contact_friction(body);
    let composed=query("QueryUsdPrim",#{doc_id:doc,path:material_root()+"/ContactSurface",topology:true});
    if composed.topology==() || composed.topology.parts[0].materials.physics!=material_root()+"/Mat" ||
        (friction!=() && abs(friction.dynamic-0.81)<0.001) {print("MATERIAL_PENDING");return;}
    report_verdict([t_true(friction==() || abs(friction.dynamic-0.81)>0.001,
        "visual-only binding leaves the collider on its ordinary contact material")],
        "NATIVE MATERIAL VISUAL COLLIDER","NATIVE_MATERIAL_VISUAL_COLLIDER");
}
fn material_physical_bind(doc) {
    if !set(material_entity(doc),"Transform.translation.x",6.0) {throw("cannot set live pose probe");}
    let edit=cmd("ApplyUsdOp",#{doc_id:doc,op:#{SetRelationship:#{edit_target:"@root@",path:material_root()+"/ContactSurface",
        name:"material:binding",targets:[material_root()+"/Alternate"]}}});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_contact_physical(doc) {
    let friction=contact_friction(find_path(material_root()+"/ContactSurface"));
    if friction==() || abs(friction.dynamic-0.81)>0.001 {print("MATERIAL_PENDING");return;}
    report_verdict([t_true(abs(friction.dynamic-0.81)<0.001 && get(material_entity(doc),"Transform.translation.x")==6.0,"all-purpose binding updates contact friction and preserves unrelated live poses")],
        "NATIVE MATERIAL COLLIDER","NATIVE_MATERIAL_COLLIDER");
}

fn material_animated_body() {material_root()+"/AnimatedBody"}
fn material_sample(path, name, kind, time, value) {
    #{SetTimeSample:#{edit_target:"@root@",path:path,name:name,type_name:kind,time:time,value:value}}
}
fn material_color_body(doc) {
    let body=material_animated_body();
    let edit=cmd("ApplyUsdOps",#{doc_id:doc,label:"Animate a dynamic body's appearance",ops:[
        material_add(material_root(),"AnimatedBody","Cube"),
        material_attr(body,"xformOp:translate","double3","(100,100,0)"),
        material_attr(body,"xformOpOrder","token[]","[\"xformOp:translate\"]"),
        #{SetApiSchemas:#{edit_target:"@root@",path:body,schemas:["PhysicsCollisionAPI","PhysicsRigidBodyAPI"]}},
        material_sample(body,"primvars:displayColor","color3f[]",0.0,"[(0.2,0.5,0.8)]"),
        material_sample(body,"primvars:displayColor","color3f[]",1.0,"[(0.7,0.1,0.3)]")
    ]});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_pose_body(doc) {
    let body=material_animated_body();
    let edit=cmd("ApplyUsdOps",#{doc_id:doc,label:"Animate motion",ops:[
        material_sample(body,"xformOp:translate","double3",0.0,"(100,100,0)"),
        material_sample(body,"xformOp:translate","double3",1.0,"(100,110,0)")
    ]});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_release_body(doc) {
    let ops=[];
    for time in [0.0,1.0] {
        ops.push(#{RemoveTimeSample:#{edit_target:"@root@",path:material_animated_body(),name:"xformOp:translate",time:time}});
    }
    let edit=cmd("ApplyUsdOps",#{doc_id:doc,label:"Release animated motion",ops:ops});
    if !edit.ok {throw(edit.to_string());}
    print("FIXTURE_APPLIED");
}
fn material_body_mode(expected) {
    let body=find_path(material_animated_body());
    if body<0 {print("MATERIAL_PENDING");return;}
    let mode=query("QueryPhysicsState",#{id:body}).body_mode;
    if mode!=expected {print("MATERIAL_PENDING");return;}
    report_verdict([t_true(mode==expected,"physics motion mode with the current animation channels is "+expected)],
        "NATIVE ANIMATION MOTION","NATIVE_ANIMATION_MOTION");
}
fn material_dynamic_body(doc) {material_body_mode("dynamic");}
fn material_kinematic_body(doc) {material_body_mode("kinematic");}
