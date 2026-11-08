// Rhai owns the Scenarios menu tree and the empty-viewport scene choices.
// Rust supplies only admitted scene, Twin, selection, and preference facts.

fn scenario_menu(menus) {
    #{ menus: menus, viewport_actions: [], navigation_entries: [],
        viewport_guidance: "Choose a scene from Scenarios, explore the demo, start a tutorial, or open a Twin." }
}

fn divider() {
    #{ separator: true }
}

fn test_visibility_setting(ctx) {
    #{ label: "Show library test scenes",
        tooltip: "Twin test scenes remain available in each Twin's Test Scenes submenu.",
        selection: "checkbox", selected: ctx.show_test_assets,
        tool: "scenarios", hook: "toggle_test_scenes",
        args: #{ show_test_assets: ctx.show_test_assets } }
}

fn sim_core_settings(ctx) {
    #{ label: "Sim Core", items: [test_visibility_setting(ctx)] }
}

fn menu_action(label, tool, hook, args) {
    #{ label: label, tool: tool, hook: hook, args: args }
}

fn scene_label(stem) {
    let label = "";
    for word in stem.split("_") {
        if word == "" { continue; }
        if label != "" { label += " "; }
        label += word.sub_string(0, 1).to_upper() + word.sub_string(1);
    }
    label
}

fn scene_item(scene, current_scene) {
    let item = #{
        label: scene_label(scene.stem),
        selection: "radio",
        selected: current_scene == scene.uri,
        tool: "scenarios",
        hook: "load",
        args: #{ uri: scene.uri, open_twin: false }
    };
    if scene.description != () { item.tooltip = scene.description; }
    item
}

fn scene_items(ctx, twin_id, test_scenes) {
    let items = [];
    for scene in ctx.scenes {
        if scene.is_test != test_scenes { continue; }
        if twin_id == () {
            if scene.twin != () { continue; }
        } else if scene.twin == () || scene.twin.id != twin_id {
            continue;
        }
        items.push(scene_item(scene, ctx.current_scene));
    }
    items
}

fn submenu(label, children) {
    #{ label: label, children: children }
}

fn twin_default_scene_item(twin, scene) {
    let args = #{};
    args.open_twin = true;
    let twin_id = twin.id;
    let twin_id_text = twin_id.to_string();
    args.twin_id = twin_id_text.parse_int();
    args.authority = twin.authority;
    args.relative_path = scene.relative_path;
    let item = menu_action("Load Default Scene", "scenarios", "load", args);
    if scene.description != () { item.tooltip = scene.description; }
    item
}

fn twin_label(twin) {
    if twin.name != () {
        let name = twin.name;
        name.trim();
        if name != "" { return twin.name; }
    }
    if twin.directory != () {
        let directory = twin.directory;
        directory.trim();
        if directory != "" { return twin.directory; }
    }
    "Project " + twin.id
}

fn viewport_action(scene, twin, primary) {
    let label = if primary { "Load default scene" } else { scene_label(scene.stem) };
    let action = #{
        label: label,
        description: if scene.description == () { scene.relative_path } else { scene.description },
        enabled: true,
        primary: primary,
        visibility: "always",
        tool: "scenarios",
        hook: "load",
        args: #{
            open_twin: true,
            twin_id: twin.id.to_string().parse_int(),
            authority: twin.authority,
            relative_path: scene.relative_path
        }
    };
    action
}

fn active_viewport_actions(ctx) {
    let actions = [];
    if ctx.active_twin == () || ctx.active_twin.authority == () || !ctx.catalog_ready
        || ctx.catalog_error != () { return actions; }
    let twin = ctx.active_twin;
    let default_scene = ();
    for scene in ctx.scenes {
        if scene.is_test || scene.twin == () || scene.twin.id != twin.id
            || scene.twin.default_scene != scene.relative_path { continue; }
        default_scene = scene;
        actions.push(viewport_action(default_scene, twin, true));
        break;
    }
    for scene in ctx.scenes {
        if scene.is_test || scene.twin == () || scene.twin.id != twin.id { continue; }
        if default_scene != () && scene.relative_path == default_scene.relative_path { continue; }
        actions.push(viewport_action(scene, twin, false));
    }
    actions
}

fn viewport_guidance(ctx) {
    if ctx.active_twin == () {
        return "Choose a scene from Scenarios, explore the demo, start a tutorial, or open a Twin.";
    }
    let twin = ctx.active_twin;
    let name = twin_label(twin);
    if twin.authority == () { return "Preparing " + name + "…"; }
    if ctx.catalog_error != () { return "Scene list unavailable: " + ctx.catalog_error; }
    if !ctx.catalog_ready { return "Loading scenes for " + name + "…"; }
    let count = 0;
    for scene in ctx.scenes {
        if !scene.is_test && scene.twin != () && scene.twin.id == twin.id { count += 1; }
    }
    if count == 0 {
        if twin.default_scene != () {
            return "The configured default scene for " + name + " is not in its admitted scene list.";
        }
        return "This Twin has no loadable scenes.";
    }
    "Choose a scene from " + name + "."
}

fn navigation_entry(scene, owner) {
    let args = #{ uri: scene.uri, open_twin: false };
    let entry = #{
        id: "scene:" + scene.uri,
        label: "Load " + scene_label(scene.stem),
        detail: owner + " / " + scene.relative_path + " · replaces the loaded scene",
        category: "Scenes",
        tool: "scenarios",
        hook: "load",
        args: args
    };
    entry
}

fn navigation_entries(ctx) {
    let entries = [];
    for scene in ctx.scenes {
        if scene.is_test && scene.twin == () && !ctx.show_test_assets { continue; }
        let owner = if scene.twin == () { "LunCo Library" } else { twin_label(scene.twin) };
        entries.push(navigation_entry(scene, owner));
    }
    entries
}

fn downloaded_twin_item(download) {
    let name = if download.name == "" { "Downloaded Twin" } else { download.name };
    let megabytes = (download.total_bytes.parse_float() / 1048576.0).to_int();
    let label = name + "  (" + megabytes.to_string() + " MB)";
    if download.default_scene == () {
        return #{ label: label + " · no default scene", enabled: false };
    }
    #{
        label: label,
        tooltip: download.default_scene,
        tool: "scenarios",
        hook: "open_downloaded",
        args: #{ scenario_id: download.scenario_id, revision: download.revision,
            name: name, scene: download.default_scene }
    }
}

fn library_groups(ctx) {
    let groups = [];
    let library = scene_items(ctx, (), false);
    if library.len() > 0 {
        let group = submenu("Library", library);
        groups.push(group);
    }
    if ctx.show_test_assets {
        let tests = scene_items(ctx, (), true);
        if tests.len() > 0 {
            if groups.len() > 0 { groups.push(divider()); }
            let group = submenu("Library tests", tests);
            groups.push(group);
        }
    }
    groups
}

fn twin_groups(ctx) {
    let groups = [];
    for twin in ctx.twins {
        let scenarios = scene_items(ctx, twin.id, false);
        let tests = scene_items(ctx, twin.id, true);
        let children = [];
        let default_scene = ();
        for scene in ctx.scenes {
            if scene.twin != () && scene.twin.id == twin.id
                && twin.default_scene == scene.relative_path {
                default_scene = scene;
                break;
            }
        }
        if default_scene != () && twin.authority != () {
            children.push(twin_default_scene_item(twin, default_scene));
            if scenarios.len() > 0 { children.push(divider()); }
        }
        for scene in scenarios { children.push(scene); }
        if tests.len() > 0 {
            if children.len() > 0 { children.push(divider()); }
            let group = submenu("Test Scenes", tests);
            children.push(group);
        }
        if children.len() > 0 {
            let group = submenu(twin_label(twin), children);
            groups.push(group);
        }
    }
    groups
}

fn downloaded_group(ctx) {
    if !ctx.download_sync_enabled { return (); }
    let downloaded = [];
    if ctx.cached_twins.len() == 0 {
        downloaded.push(#{ label: "Connect to a server to download one", enabled: false });
    } else {
        for twin in ctx.cached_twins { downloaded.push(downloaded_twin_item(twin)); }
    }
    let label = "Downloaded Twins (" + ctx.cached_twins.len().to_string() + ")";
    submenu(label, downloaded)
}

fn application_scenarios_menu(ctx) {
    let menus = [];
    let primary_items = [];
    let restart = menu_action("Restart Scenario", "scenarios", "restart", #{});
    restart.enabled = ctx.current_scene != ();
    primary_items.push(restart);
    let twins = twin_groups(ctx);
    if primary_items.len() > 0 && twins.len() > 0 { primary_items.push(divider()); }
    let twin_index = 0;
    for twin in twins {
        if twin_index > 0 { primary_items.push(divider()); }
        primary_items.push(twin);
        twin_index += 1;
    }
    if primary_items.len() > 0 {
        menus.push(#{ label: "Scenarios", order: -100, items: primary_items });
    }

    let library_items = [];
    if ctx.catalog_error != () {
        library_items.push(#{ label: "Scenarios unavailable", enabled: false,
            tooltip: ctx.catalog_error });
    } else if !ctx.catalog_ready {
        library_items.push(#{ label: "Loading scenarios…", enabled: false });
    } else {
        for group in library_groups(ctx) { library_items.push(group); }
    }

    let downloaded = downloaded_group(ctx);
    if downloaded != () {
        if library_items.len() > 0 { library_items.push(divider()); }
        library_items.push(downloaded);
    }
    if library_items.len() == 0 && primary_items.len() == 0 {
        library_items.push(#{ label: "No scenes found", enabled: false });
    }
    if library_items.len() > 0 {
        menus.push(#{ label: "Scenarios", order: 100, items: library_items });
    }

    let result = scenario_menu(menus);
    result.settings_submenus = [sim_core_settings(ctx)];
    result.viewport_actions = active_viewport_actions(ctx);
    if ctx.active_twin == () {
        let demo_args = #{ uri: "lunco://scenes/luncosim/sandbox_scene.usda", open_twin: false };
        result.viewport_actions.push(#{
            label: "Explore demo sandbox",
            description: "Explore the collection of vehicle and physics demonstrations.",
            enabled: true,
            primary: false,
            tool: "scenarios",
            hook: "load",
            args: demo_args
        });
    }
    result.viewport_guidance = viewport_guidance(ctx);
    result.navigation_entries = navigation_entries(ctx);
    result
}
