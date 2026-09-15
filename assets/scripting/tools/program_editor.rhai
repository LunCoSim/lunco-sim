// Generic authored-program editor.
//
// USD owns the program prim and its selected source arm. This library only
// composes the existing typed query/command surfaces; it knows nothing about
// vessels, routes, autopilots, or model classes. Twin policy decides which
// program a UI action addresses and which source is appropriate.

fn failure(message) {
    notify_kind("Program edit failed: " + message, "error");
    #{ ok: false, error: message }
}

fn document_generation(doc) {
    let result = query("InspectUsdDocument", #{ doc_id: doc });
    if result == () || result.ok == false || result.generation == () {
        return ();
    }
    result.generation
}

fn target(doc, path) {
    let result = query("ResolveUsdTarget", #{
        doc_id: doc,
        path: path,
        edit_target: "@root@",
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "USD target query returned no result" } else { result.error } };
    }
    #{
        ok: true,
        exists: result.composed_exists == true || result.authored_in_document == true,
        authored_here: result.authored_here == true,
        under_arc: result.under_arc == true,
    }
}

fn edit_target(doc, path) {
    let resolved = target(doc, path);
    if !resolved.ok || !resolved.exists {
        return #{ ok: false, error: "program prim does not exist in the document" };
    }
    // A composed/reference child is overridden in the document root. A prim
    // authored in the runtime layer stays there; both are explicit USD edits.
    #{
        ok: true,
        layer: if resolved.authored_here || resolved.under_arc { "@root@" } else { "@runtime@" },
    }
}

fn program_record(doc, path) {
    let result = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: [
            "info:implementationSource",
            "info:id",
            "info:sourceAsset",
            "info:sourceCode",
        ],
        schemas: true,
    });
    if result == () || result.ok == false {
        return #{ ok: false, error: if result == () { "program query returned no result" } else { result.error } };
    }
    let schemas = result.api_schemas;
    let is_program = false;
    if schemas != () {
        for schema in schemas {
            if schema == "LunCoProgramAPI" { is_program = true; break; }
        }
    }
    if !is_program {
        return #{ ok: false, error: "prim does not apply LunCoProgramAPI" };
    }
    #{
        ok: true,
        path: path,
        implementation_source: result.attrs["info:implementationSource"],
        id: result.attrs["info:id"],
        source_asset: result.attrs["info:sourceAsset"],
        source_code: result.attrs["info:sourceCode"],
    }
}

fn emit_selected(path) {
    let record = query("QueryUsdPrim", #{ path: path, rels: ["inputs:subject"] });
    let subjects = if record != () && record.ok != false
        && record.relationships != () { record.relationships["inputs:subject"] } else { [] };
    emit("program.selected", #{
        path: path,
        subject: if subjects != () && subjects.len() == 1 { subjects[0] } else { () },
    });
}

fn list(doc, scope) {
    let result = query("QueryUsdPrim", #{ doc_id: doc, path: scope, children: true });
    if result == () || result.ok == false || result.children == () {
        return #{ ok: false, error: "program scope is not synchronized" };
    }
    let programs = [];
    for child in result.children {
        let record = program_record(doc, child);
        if record.ok { programs.push(record); }
    }
    #{ ok: true, scope: scope, programs: programs }
}

fn valid_source_asset(source) {
    if source == () || source == "" || source.contains("@") || source.starts_with("/") {
        return false;
    }
    for part in source.split("/") {
        if part == ".." { return false; }
    }
    source.ends_with(".rhai") || source.ends_with(".mo") || source.ends_with(".py")
}

fn switch_source(doc, path, source) {
    if !valid_source_asset(source) {
        return failure("program source must be a relative .rhai, .mo, or .py asset path");
    }
    let record = program_record(doc, path);
    if !record.ok { return failure(record.error); }
    let layer = edit_target(doc, path);
    if !layer.ok { return failure(layer.error); }
    let ops = [
        // Select the new arm last. The previous arm remains selected until all
        // conflicting opinions have been cleared in the same USD transaction.
        #{ SetAttribute: #{
            edit_target: layer.layer,
            path: path,
            name: "info:sourceAsset",
            type_name: "asset",
            value: "@" + source + "@",
        }},
        #{ SetAttribute: #{
            edit_target: layer.layer,
            path: path,
            name: "info:id",
            type_name: "token",
            value: "\"\"",
        }},
        #{ SetAttribute: #{
            edit_target: layer.layer,
            path: path,
            name: "info:sourceCode",
            type_name: "string",
            value: "",
        }},
        #{ SetAttribute: #{
            edit_target: layer.layer,
            path: path,
            name: "info:implementationSource",
            type_name: "token",
            value: "\"sourceAsset\"",
        }},
    ];
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: document_generation(doc),
        label: "Switch authored program source",
        ops: ops,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "USD program source edit returned no result" } else { result.error });
    }
    // A source swap recreates the program instance. Re-emit the generic
    // selection edge so a selected program stays selected across hot reload.
    emit_selected(path);
    #{ ok: true, path: path, source_asset: source, generation: result.generation }
}

fn select(path) {
    let entity_id = find_path(path);
    if entity_id < 0 { return failure("program has no live projection"); }
    // Program selection is ephemeral control-plane state. Keep it in the
    // shared scene-selection owner instead of authoring a runtime USD prim:
    // selecting a program must not advance the document generation or make
    // canonical doc_id reads wait for a structural projection.
    let result = cmd("SelectSceneEntity", #{
        entity_id: entity_id,
        extend: false,
        toggle: false,
        remove_only: false,
    });
    if result == () || result.ok == false {
        return failure(if result == () { "SelectSceneEntity command is unavailable in this host" } else { result.error });
    }
    emit_selected(path);
    #{ ok: true, path: path, entity_id: entity_id }
}

fn edit(path) {
    let selected = select(path);
    if !selected.ok { return selected; }
    if !is_unattended() { cmd("FocusPanel", #{ id: "rhai_editor" }); }
    selected
}

fn add(doc, host, name, source) {
    if !valid_source_asset(source) {
        return failure("program source must be a relative .rhai, .mo, or .py asset path");
    }
    if name == () || name == "" {
        return failure("program name is required");
    }
    let result = assembly_edit::attach_program(doc, #{
        edit_target: "@root@",
        host_path: host,
        name: name,
        source_asset: source,
        inputs: [],
        outputs: [],
        realtime_safe: false,
    });
    if result == () || result.ok != true {
        return failure(if result == () { "AttachProgram returned no result" } else { result.error });
    }
    #{ ok: true, path: host + "/" + name, source_asset: source }
}

/// Opt one authored prim into the reusable program-browser surface. The
/// surface identity and visibility policy are ordinary USD UI metadata; a Twin
/// or its Rhai scenario decides when this is appropriate for any model.
fn enable_hud(doc, scope, visibility) {
    if visibility != "always" && visibility != "possessed" {
        return failure("program HUD visibility must be `always` or `possessed`");
    }
    let resolved = target(doc, scope);
    if !resolved.ok || !resolved.exists { return failure("program HUD scope does not exist"); }
    let layer = if resolved.authored_here || resolved.under_arc { "@root@" } else { "@runtime@" };
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: document_generation(doc),
        label: "Enable program browser surface",
        ops: [
            #{ SetApiSchemas: #{
                edit_target: layer,
                path: scope,
                schemas: ["LunCoUiSchemaAPI"],
            }},
            #{ SetAttribute: #{
                edit_target: layer,
                path: scope,
                name: "lunco:ui:surfaceId",
                type_name: "string",
                value: "program-browser",
            }},
            #{ SetAttribute: #{
                edit_target: layer,
                path: scope,
                name: "lunco:ui:visibilityMode",
                type_name: "string",
                value: visibility,
            }},
        ],
    });
    if result == () || result.ok != true {
        return failure(if result == () { "program HUD authoring returned no result" } else { result.error });
    }
    #{ ok: true, scope: scope, surface: "program-browser", visibility: visibility }
}

fn disable_hud(doc, scope) {
    let resolved = target(doc, scope);
    if !resolved.ok || !resolved.exists { return failure("program HUD scope does not exist"); }
    let layer = if resolved.authored_here || resolved.under_arc { "@root@" } else { "@runtime@" };
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc,
        parent_gen: document_generation(doc),
        label: "Disable program browser surface",
        ops: [#{ SetAttribute: #{
            edit_target: layer,
            path: scope,
            name: "lunco:ui:surfaceId",
            type_name: "string",
            value: "",
        }}],
    });
    if result == () || result.ok != true {
        return failure(if result == () { "program HUD disable returned no result" } else { result.error });
    }
    #{ ok: true, scope: scope, surface: "" }
}
