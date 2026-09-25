// Application-wide lifecycle policy. A Twin can replace this hook from its
// own policy manifest without changing the Rust runtime.
fn twin_path(path) {
    let normalized = "";
    for part in path.split("\\") {
        if normalized != "" { normalized += "/"; }
        normalized += part;
    }
    normalized
}

fn twin_default_scene(manifest) {
    if manifest == () { return (); }
    if !manifest.contains("usd") { return (); }
    let usd = manifest["usd"];
    if usd == () { return (); }
    if !usd.contains("default_scene") { return (); }
    twin_path(usd["default_scene"])
}

fn twin_under_any_root(path, roots) {
    path = twin_path(path);
    for root in roots {
        let normalized_root = twin_path(root);
        if normalized_root == "." || path == normalized_root || path.starts_with(normalized_root + "/") { return true; }
    }
    false
}

fn twin_parent_path(path) {
    path = twin_path(path);
    let parts = path.split("/");
    if parts.len() < 2 { return "."; }
    let parent = "";
    for index in 0..(parts.len() - 1) {
        if index > 0 { parent += "/"; }
        parent += parts[index];
    }
    parent
}

fn twin_command(command_name, params) {
    let action = #{};
    action.command = command_name;
    action.params = params;
    action
}

fn twin_scene_actions(ctx) {
    let actions = [];
    let scene = twin_default_scene(ctx.manifest);
    if scene != () {
        let params = #{};
        params.twin_id = ctx.twin_id;
        params.name = ctx.name;
        params.relative_path = scene;
        actions.push(twin_command("OpenTwinScene", params));
    } else {
        let reason = ctx.root;
        if ctx.manifest == () {
            reason += " has no twin.toml, so there is no scene to load. You may have opened the wrong folder; open the Twin root containing twin.toml.";
        } else {
            reason += " has a twin.toml but declares no default scene — nothing to load.";
        }
        let params = #{};
        params.reason = reason;
        actions.push(twin_command("SetEmptyViewportReason", params));
        actions.push(twin_command("ClearScene", #{}));
    }
    actions
}

fn twin_tool_actions(ctx) {
    let actions = [];
    let scope_params = #{};
    scope_params.twin_id = ctx.twin_id;
    actions.push(twin_command("ActivateTwinToolScope", scope_params));
    for path in ctx.files {
        let in_tools = twin_parent_path(path) == "tools";
        let is_rhai = path.ends_with(".rhai");
        if in_tools && is_rhai {
            let parts = path.split("/");
            let final_index = parts.len() - 1;
            let filename = parts[final_index];
            let names = filename.split(".rhai");
            let library_name = names[0];
            let params = #{};
            params.twin_id = ctx.twin_id;
            params.name = ctx.name;
            params.library_name = library_name;
            params.relative_path = path;
            actions.push(twin_command("LoadTwinToolLibrary", params));
        }
    }
    actions
}

fn twin_timeline_actions(ctx) {
    let actions = [];
    let scope_params = #{};
    scope_params.twin_id = ctx.twin_id;
    actions.push(twin_command("ActivateTwinTimelineScope", scope_params));
    for path in ctx.files {
        let in_timelines = twin_parent_path(path) == "timelines";
        let is_json = path.ends_with(".json");
        if in_timelines && is_json {
            let parts = path.split("/");
            let final_index = parts.len() - 1;
            let filename = parts[final_index];
            let names = filename.split(".json");
            let timeline_name = names[0];
            let params = #{};
            params.twin_id = ctx.twin_id;
            params.name = ctx.name;
            params.timeline_name = timeline_name;
            params.relative_path = path;
            actions.push(twin_command("LoadTwinTimelineFile", params));
        }
    }
    actions
}

fn twin_sysml_config(manifest) {
    let sysml = ();
    if manifest != () {
        if manifest.contains("sysml") {
            sysml = manifest["sysml"];
        }
    }
    sysml
}

fn twin_sysml_roots(sysml) {
    let roots = ["."];
    if sysml != () {
        if sysml.contains("paths") {
            roots = sysml["paths"];
        }
    }
    if roots.len() == 0 {
        roots = ["."];
    }
    roots
}

fn twin_sysml_root(sysml) {
    let root = ();
    if sysml != () {
        if sysml.contains("root") {
            root = twin_path(sysml["root"]);
        }
    }
    root
}

fn twin_sysml_sources(ctx, roots) {
    let sources = [];
    for path in ctx.files {
        let has_sysml_extension = path.ends_with(".sysml") || path.ends_with(".kerml");
        if has_sysml_extension {
            if twin_under_any_root(path, roots) {
                sources.push(path);
            }
        }
    }
    sources
}

fn twin_sysml_source_action(ctx, path) {
    let params = #{};
    params.twin_id = ctx.twin_id;
    params.name = ctx.name;
    params.relative_path = path;
    twin_command("LoadTwinSysmlSource", params)
}

fn twin_sysml_analysis_action(ctx, sources) {
    let params = #{};
    params.twin_id = ctx.twin_id;
    params.name = ctx.name;
    params.relative_paths = sources;
    twin_command("PrepareTwinSysmlAnalysis", params)
}

fn twin_sysml_primary_action(ctx, root, roots, sources) {
    if root == () { return (); }
    if !twin_under_any_root(root, roots) { return (); }
    if !sources.contains(root) { return (); }
    twin_sysml_source_action(ctx, root)
}

fn twin_sysml_actions(ctx, manifest) {
    let sysml = twin_sysml_config(manifest);
    let roots = twin_sysml_roots(sysml);
    let root = twin_sysml_root(sysml);
    let sources = twin_sysml_sources(ctx, roots);
    let source_count = sources.len();
    let analysis_sources = sources;
    let actions = [];
    let root_action = twin_sysml_primary_action(ctx, root, roots, sources);
    if root_action != () {
        actions.push(root_action);
    }
    for path in sources {
        if path != root {
            let action = twin_sysml_source_action(ctx, path);
            actions.push(action);
        }
    }
    actions.push(twin_sysml_analysis_action(ctx, analysis_sources));
    let info = "";
    if source_count == 0 {
        info = "Twin " + ctx.name + " declares no indexed .sysml or .kerml sources";
    }
    let plan = #{};
    plan.actions = actions;
    plan.info = info;
    plan
}

fn twin_modelica_config(manifest) {
    let modelica = ();
    if manifest != () {
        if manifest.contains("modelica") {
            modelica = manifest["modelica"];
        }
    }
    modelica
}

fn twin_modelica_root_covers(root, candidate) {
    if root == "." { return true; }
    let prefix = root + "/";
    candidate.starts_with(prefix)
}

fn twin_modelica_root_is_nested(root, candidate) {
    let prefix = candidate + "/";
    root.starts_with(prefix)
}

fn twin_modelica_candidate_is_nested(candidate, roots) {
    for root in roots {
        if twin_modelica_root_covers(root, candidate) {
            return true;
        }
    }
    false
}

fn twin_modelica_remove_nested_roots(roots, candidate) {
    let retained = [];
    for root in roots {
        if !twin_modelica_root_is_nested(root, candidate) {
            retained.push(root);
        }
    }
    retained
}

fn twin_modelica_roots_from_files(ctx) {
    let candidates = [];
    for path in ctx.files {
        if path.ends_with(".mo") {
            let candidate = twin_parent_path(path);
            if !candidates.contains(candidate) {
                candidates.push(candidate);
            }
        }
    }
    let roots = [];
    for candidate in candidates {
        if !twin_modelica_candidate_is_nested(candidate, roots) {
            if candidate == "." {
                roots = ["."];
            } else {
                let retained = twin_modelica_remove_nested_roots(roots, candidate);
                retained.push(candidate);
                roots = retained;
            }
        }
    }
    roots
}

fn twin_modelica_manifest_roots(modelica) {
    let roots = [];
    for path in modelica["paths"] {
        roots.push(twin_path(path));
    }
    roots
}

fn twin_modelica_local_roots(ctx, modelica) {
    if modelica != () {
        if modelica.contains("paths") {
            return twin_modelica_manifest_roots(modelica);
        }
    }
    twin_modelica_roots_from_files(ctx)
}

fn twin_modelica_local_actions(ctx, roots, source_index) {
    let actions = [];
    for path in roots {
        let params = #{};
        params.twin_id = ctx.twin_id;
        params.name = ctx.name;
        params.id = "twin:" + ctx.name + ":modelica:" + source_index.to_string();
        params.path = path;
        actions.push(twin_command("LoadTwinModelicaSourceRoot", params));
        source_index += 1;
    }
    let plan = #{};
    plan.actions = actions;
    plan.next_index = source_index;
    plan
}

fn twin_modelica_external_actions(ctx, modelica, source_index) {
    let actions = [];
    if modelica == () { return actions; }
    if !modelica.contains("externals") { return actions; }
    for external in modelica["externals"] {
        let path = twin_path(external.path);
        if !path.starts_with("@bundled:") {
            let params = #{};
            params.twin_id = ctx.twin_id;
            params.name = ctx.name;
            params.id = "twin:" + ctx.name + ":modelica:" + source_index.to_string();
            params.path = path;
            actions.push(twin_command("LoadTwinModelicaSourceRoot", params));
            source_index += 1;
        }
    }
    actions
}

fn twin_modelica_actions(ctx, manifest) {
    let modelica = twin_modelica_config(manifest);
    let roots = twin_modelica_local_roots(ctx, modelica);
    let local_plan = twin_modelica_local_actions(ctx, roots, 0);
    let actions = [];
    for action in local_plan.actions {
        actions.push(action);
    }
    let external_actions = twin_modelica_external_actions(ctx, modelica, local_plan.next_index);
    for action in external_actions {
        actions.push(action);
    }
    actions
}

fn twin_lifecycle(event, ctx) {
    let actions = [];
    let info = "";
    if event == "assets_mounted" && ctx.active {
        let scene_actions = twin_scene_actions(ctx);
        for action in scene_actions {
            actions.push(action);
        }
        let tool_actions = twin_tool_actions(ctx);
        for action in tool_actions {
            actions.push(action);
        }
        let timeline_actions = twin_timeline_actions(ctx);
        for action in timeline_actions {
            actions.push(action);
        }
        let sysml_plan = twin_sysml_actions(ctx, ctx.manifest);
        for action in sysml_plan.actions {
            actions.push(action);
        }
        info = sysml_plan.info;
        let modelica_actions = twin_modelica_actions(ctx, ctx.manifest);
        for action in modelica_actions {
            actions.push(action);
        }
    }
    let result = #{};
    result.event = event;
    result.twin_id = ctx.twin_id;
    result.root = ctx.root;
    result.scope = ctx.scope;
    result.installed = ctx.installed;
    result.failed = ctx.failed;
    result.actions = actions;
    result.info = info;
    result
}
