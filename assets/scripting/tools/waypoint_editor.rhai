// Waypoint authoring policy.
//
// The editor supplies one explicit operation context and this tool lowers it
// to the existing generic USD operation command. Rust owns pointer geometry,
// document identity, and projection; this file owns the route policy. It is
// therefore hot-reloadable and can evolve without adding a waypoint command or
// another USD writer.

fn ui_label() { "Waypoint editor" }
fn ui_hint() { "Select a vessel, then click terrain to append a waypoint" }

const MARKER_ASSET = "lunco://vessels/markers/waypoint.usda";
const ROOT_LAYER = "@root@";

fn failure(message) {
    notify_kind("Waypoint edit failed: " + message, "error");
    #{ ok: false, error: message }
}

fn resolved(doc, path) {
    let result = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: ROOT_LAYER,
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "USD target query returned no result" } else { result.error } };
    }
    #{
        ok: true,
        exists: result.composed_exists == true || result.authored_in_document == true,
    }
}

fn parent_generation(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        children: false,
    });
    if result == () || result.ok == false || result.generation == () {
        return ();
    }
    result.generation
}

fn source_ops(mission, xml) {
    [
        #{ SetAttribute: #{
            edit_target: ROOT_LAYER,
            path: mission,
            name: "info:sourceCode",
            type_name: "string",
            value: xml,
        }},
        #{ SetAttribute: #{
            edit_target: ROOT_LAYER,
            path: mission,
            name: "info:id",
            type_name: "token",
            value: "\"\"",
        }},
        #{ SetAttribute: #{
            edit_target: ROOT_LAYER,
            path: mission,
            name: "info:sourceAsset",
            type_name: "asset",
            value: "@@",
        }},
        #{ SetAttribute: #{
            edit_target: ROOT_LAYER,
            path: mission,
            name: "info:implementationSource",
            type_name: "token",
            value: "\"sourceCode\"",
        }},
    ]
}

fn load_route(doc, mission, supplied_xml) {
    if supplied_xml != "" {
        let imported = cmd("ImportBehaviorXml", #{ xml: supplied_xml });
        if imported == () || imported.ok != true || imported.data == ()
            || imported.data.spec_json == () {
            return failure("the current mission XML could not be imported");
        }
        let spec = from_json(imported.data.spec_json);
        if spec == () { return failure("the current mission JSON is invalid"); }
        return #{ ok: true, spec: spec };
    }

    let existing = resolved(doc, mission);
    if !existing.ok { return failure(existing.error); }
    if !existing.exists {
        return #{ ok: true, spec: #{ kind: "sequence", children: [] } };
    }

    let record = query("QueryUsdPrim", #{
        doc_id: doc,
        path: mission,
        attrs: ["info:sourceCode", "info:sourceAsset"],
        children: false,
    });
    if record == () || record.ok == false || record.attrs == () {
        return failure("the existing mission is not readable yet; wait for projection");
    }
    let source_code = record.attrs["info:sourceCode"];
    let source_asset = record.attrs["info:sourceAsset"];
    if source_code != () && source_code != "" {
        let imported = cmd("ImportBehaviorXml", #{ xml: source_code });
        if imported == () || imported.ok != true || imported.data == () {
            return failure("the existing mission XML could not be imported");
        }
        let spec = from_json(imported.data.spec_json);
        if spec != () { return #{ ok: true, spec: spec }; }
        return failure("the existing mission JSON is invalid");
    }
    if source_asset != () && source_asset != "" && source_asset != "@@" {
        return failure("the mission file is still loading; wait until its route is ready");
    }
    failure("the existing mission has no readable source");
}

fn route_children(spec) {
    if spec.kind == "sequence" && spec.children != () {
        return spec.children;
    }
    if spec.kind == "forever" && spec.child != ()
        && spec.child.kind == "sequence" && spec.child.children != () {
        return spec.child.children;
    }
    ()
}

fn with_route_children(spec, children) {
    if spec.kind == "sequence" {
        spec.children = children;
        return spec;
    }
    if spec.kind == "forever" && spec.child != () && spec.child.kind == "sequence" {
        spec.child.children = children;
        return spec;
    }
    ()
}

fn export_route(spec) {
    let exported = cmd("ExportBehaviorXml", #{ spec_json: to_json(spec) });
    if exported == () || exported.ok != true || exported.data == ()
        || exported.data.xml == () {
        return #{ ok: false, error: "the edited route could not be exported" };
    }
    #{ ok: true, xml: exported.data.xml }
}

fn apply_ops(doc, vessel_path, label, ops) {
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: parent_generation(doc, vessel_path),
        label: label,
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD edit returned no result" } else { result.error });
    }
    #{ ok: true }
}

fn marker_name(doc, route_scope) {
    let index = 0;
    while index < 100000 {
        let name = "W" + index.to_string();
        let target = resolved(doc, route_scope + "/" + name);
        if !target.ok { return target; }
        if !target.exists { return #{ ok: true, name: name }; }
        index += 1;
    }
    #{ ok: false, error: "route contains too many waypoint names" }
}

fn ensure_marker(doc, root, position) {
    let route_scope = root + "/Route";
    let route = resolved(doc, route_scope);
    if !route.ok { return route; }
    let name = marker_name(doc, route_scope);
    if !name.ok { return name; }
    let path = route_scope + "/" + name.name;
    let ops = [];
    if !route.exists {
        ops.push(#{ AddPrim: #{
            edit_target: ROOT_LAYER,
            parent_path: root,
            name: "Route",
            type_name: "Scope",
            reference: "",
            reference_prim_path: "",
        }});
    }
    ops.push(#{ AddPrim: #{
        edit_target: ROOT_LAYER,
        parent_path: route_scope,
        name: name.name,
        type_name: "Xform",
        reference: MARKER_ASSET,
        reference_prim_path: "",
    }});
    ops.push(#{ SetTranslate: #{
        edit_target: ROOT_LAYER,
        path: path,
        value: position,
    }});
    #{ ok: true, path: path, ops: ops }
}

fn mission_setup(doc, vessel_path) {
    let mission = vessel_path + "/Mission";
    let result = resolved(doc, mission);
    if !result.ok { return #{ ok: false, error: result.error }; }
    let ops = [];
    if !result.exists {
        ops.push(#{ AddPrim: #{
            edit_target: ROOT_LAYER,
            parent_path: vessel_path,
            name: "Mission",
            type_name: "Scope",
            reference: "",
            reference_prim_path: "",
        }});
        ops.push(#{ SetApiSchemas: #{
            edit_target: ROOT_LAYER,
            path: mission,
            schemas: ["LunCoProgramAPI"],
        }});
    }
    #{ ok: true, path: mission, ops: ops }
}

fn edit(args) {
    if args == () || args.doc == () || args.vessel_path == ()
        || args.action == () {
        return failure("incomplete waypoint edit context");
    }
    let doc = args.doc;
    let vessel_path = args.vessel_path;
    let action = args.action;
    let target = args.target;
    let position = args.position;
    let mission = vessel_path + "/Mission";

    if action == "move" {
        if target == () || position == () { return failure("move requires target and position"); }
        return apply_ops(doc, vessel_path, "Move waypoint", [#{ SetTranslate: #{
            edit_target: ROOT_LAYER,
            path: target,
            value: position,
        }}]);
    }

    let loaded = load_route(doc, mission, args.xml);
    if !loaded.ok { return loaded; }
    let children = route_children(loaded.spec);
    if children == () {
        return failure("mission is not a plain sequence or forever(sequence) route");
    }

    if action == "append" || action == "insert_after" {
        if position == () { return failure("new waypoint requires a position"); }
        if action == "insert_after" && target == () {
            return failure("insert-after requires a target");
        }
        let marker = ensure_marker(doc, args.root, position);
        if !marker.ok { return failure(marker.error); }
        let leaf = #{ kind: "drive_to", target: marker.path };
        if action == "append" {
            children.push(leaf);
        } else {
            let at = -1;
            for i in 0..children.len() {
                if children[i].kind == "drive_to" && children[i].target == target {
                    at = i;
                    break;
                }
            }
            if at < 0 { return failure("target is not in the mission route"); }
            children.insert(at + 1, leaf);
        }
        let updated = with_route_children(loaded.spec, children);
        let exported = export_route(updated);
        if !exported.ok { return failure(exported.error); }
        let setup = mission_setup(doc, vessel_path);
        if !setup.ok { return failure(setup.error); }
        for op in setup.ops { marker.ops.push(op); }
        for op in source_ops(setup.path, exported.xml) { marker.ops.push(op); }
        return apply_ops(doc, vessel_path,
            if action == "append" { "Create waypoint mission" } else { "Insert waypoint" },
            marker.ops);
    }

    if action == "delete" || action == "dwell" || action == "smooth" {
        let updated_children = [];
        let found = false;
        if action == "delete" {
            for child in children {
                if child.kind == "drive_to" && child.target == target {
                    found = true;
                } else {
                    updated_children.push(child);
                }
            }
        } else {
            for i in 0..children.len() {
                let child = children[i];
                if child.kind == "drive_to"
                    && (action == "smooth" || child.target == target) {
                    found = found || action == "smooth" || child.target == target;
                    if action == "dwell" {
                        if args.dwell > 0.0 { child.dwell = args.dwell; }
                        else { child.remove("dwell"); }
                    } else if args.smooth { child.smooth = true; }
                    else { child.remove("smooth"); }
                }
                updated_children.push(child);
            }
        }
        if !found { return failure("target is not in the mission route"); }
        let updated = with_route_children(loaded.spec, updated_children);
        let exported = export_route(updated);
        if !exported.ok { return failure(exported.error); }
        let setup = mission_setup(doc, vessel_path);
        if !setup.ok { return failure(setup.error); }
        let ops = setup.ops;
        for op in source_ops(setup.path, exported.xml) { ops.push(op); }
        if action == "delete" {
            if target == () { return failure("delete requires a target"); }
            ops.push(#{ SetActive: #{
                edit_target: ROOT_LAYER,
                path: target,
                active: false,
            }});
        }
        return apply_ops(doc, vessel_path,
            if action == "delete" { "Delete waypoint" }
            else if action == "dwell" { "Edit waypoint dwell" }
            else { "Edit route smoothing" },
            ops);
    }
    failure("unknown waypoint edit action: " + action)
}

// Generic click-tool entry point. Rust supplies only the canonical scene
// context and the editor's current intent; route policy stays in this library.
fn on_click(context) {
    if context.action == () {
        return failure("waypoint editor click has no editing intent");
    }
    edit(context)
}
