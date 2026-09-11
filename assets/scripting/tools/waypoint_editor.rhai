// Authored route editing policy.
//
// Rust supplies only the generic click context and the typed USD-operation
// command. This policy creates ordinary USD prims and a generic Rhai program;
// no route, waypoint, or vehicle type is known by the engine.

fn ui_label() { "Route point" }
fn ui_hint() { "Click the terrain to append a point to the active route" }

const MARKER_ASSET = "lunco://markers/route_point.usda";
const ROOT_LAYER = "@root@";
const ROUTE_PROGRAM = "lunco://scenarios/route_follow.rhai";

fn failure(message) {
    notify_kind("Route edit failed: " + message, "error");
    #{ ok: false, error: message }
}

fn target_record(doc, path) {
    let result = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: ROOT_LAYER,
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "USD target query returned no result" } else { result.error } };
    }
    #{ ok: true, exists: result.composed_exists == true || result.authored_in_document == true }
}

fn generation(doc, path) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: path });
    if result == () || result.ok == false || result.generation == () {
        return ();
    }
    result.generation
}

fn apply_ops(doc, generation_path, label, ops) {
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: generation(doc, generation_path),
        label: label,
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD edit returned no result" } else { result.error });
    }
    #{ ok: true }
}

fn route_path(context) {
    if context.route_path != () && context.route_path != "" {
        return context.route_path;
    }
    if context.scene_root_path == () || context.scene_root_path == "" {
        return ();
    }
    context.scene_root_path + "/Route"
}

fn subject_path(context) {
    if context.controlled_path != () && context.controlled_path != "" {
        return context.controlled_path;
    }
    if context.selected_path != () && context.selected_path != "" {
        return context.selected_path;
    }
    ()
}

fn next_point_name(doc, route) {
    let children = query("QueryUsdPrim", #{
        doc_id: doc,
        path: route,
        children: true,
    });
    if children == () || children.ok == false || children.children == () {
        return "P0";
    }
    let index = 0;
    while index < 100000 {
        let candidate = "P" + index.to_string();
        let used = false;
        for child in children.children {
            if child == route + "/" + candidate { used = true; break; }
        }
        if !used { return candidate; }
        index += 1;
    }
    ()
}

fn route_setup(doc, route, subject) {
    let existing = target_record(doc, route);
    if !existing.ok { return existing; }
    let ops = [];
    if !existing.exists {
        if route.index_of("/") < 0 {
            return #{ ok: false, error: "route path must be rooted" };
        }
        let slash = route.last_index_of("/");
        let parent = route.sub_string(0, slash);
        let name = route.sub_string(slash + 1);
        ops.push(#{ AddPrim: #{
            edit_target: ROOT_LAYER,
            parent_path: parent,
            name: name,
            type_name: "Scope",
            reference: "",
            reference_prim_path: "",
        }});
    }
    let program = route + "/Program";
    let program_record = target_record(doc, program);
    if !program_record.ok { return program_record; }
    if !program_record.exists {
        ops.push(#{ AddPrim: #{
            edit_target: ROOT_LAYER,
            parent_path: route,
            name: "Program",
            type_name: "Scope",
            reference: "",
            reference_prim_path: "",
        }});
    }
    ops.push(#{ SetApiSchemas: #{
        edit_target: ROOT_LAYER,
        path: program,
        schemas: ["LunCoProgramAPI"],
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "info:implementationSource",
        type_name: "token",
        value: "\"sourceAsset\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "info:id",
        type_name: "token",
        value: "\"\"",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "info:sourceCode",
        type_name: "string",
        value: "",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "inputs:enabled",
        type_name: "float",
        value: "1.0",
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "info:sourceAsset",
        type_name: "asset",
        value: "@lunco://scenarios/route_follow.rhai@",
    }});
    ops.push(#{ SetRelationship: #{
        edit_target: ROOT_LAYER,
        path: program,
        name: "inputs:subject",
        targets: [subject],
    }});
    #{ ok: true, ops: ops }
}

fn marker_ops(doc, route, position) {
    let name = next_point_name(doc, route);
    if name == () { return #{ ok: false, error: "route contains too many points" }; }
    let path = route + "/" + name;
    let zone = "route_" + name;
    #{ ok: true, path: path, ops: [
        #{ AddPrim: #{
            edit_target: ROOT_LAYER,
            parent_path: route,
            name: name,
            type_name: "Xform",
            reference: MARKER_ASSET,
            reference_prim_path: "",
        }},
        #{ SetTranslate: #{
            edit_target: ROOT_LAYER,
            path: path,
            value: position,
        }},
        #{ SetAttribute: #{
            edit_target: ROOT_LAYER,
            path: path + "/Trigger",
            name: "lunco:triggerZone",
            type_name: "token",
            value: "\"" + zone + "\"",
        }},
    ] }
}

fn on_click(context) {
    if context == () || context.doc_id == () || context.scene_root_path == () {
        return failure("click has no active document or scene root");
    }
    let subject = subject_path(context);
    if subject == () { return failure("select or possess the object that owns this route"); }
    if context.world_position == () { return failure("click has no world position"); }
    let route = route_path(context);
    if route == () { return failure("active scene root has no route path"); }
    let setup = route_setup(context.doc_id, route, subject);
    if !setup.ok { return failure(setup.error); }
    let point = marker_ops(context.doc_id, route, context.world_position);
    if !point.ok { return failure(point.error); }
    let ops = setup.ops;
    for op in point.ops { ops.push(op); }
    apply_ops(context.doc_id, context.scene_root_path, "Append route point", ops)
}
