// Application policy for authored JSON menus and scene-selected dataset text.
// Rust supplies lifecycle facts and validates generic asset actions.
const TUTORIAL_CATALOG_KIND = "lunco.tutorial-catalog.v1";

fn empty_menu_contribution() {
    #{ menus: [] }
}

fn tutorial_menu_contribution(items) {
    let menu = #{};
    menu.label = "Tutorials";
    menu.items = items;
    let menus = [];
    menus.push(menu);
    let result = #{};
    result.menus = menus;
    result
}

fn loading_menu_items(ctx) {
    let items = [];
    let loading = #{};
    loading.label = "Loading…";
    loading.enabled = false;
    items.push(loading);
    if ctx.scope == "twin" {
        let wrapper = #{};
        wrapper.label = ctx.twin_name;
        wrapper.children = items;
        let wrapped = [];
        wrapped.push(wrapper);
        return wrapped;
    }
    items
}

fn blank_text(text) {
    let value = text;
    value.trim();
    value == ""
}

fn qualify_catalog_asset(root, reference) {
    if blank_text(reference) {
        return "";
    }
    if reference.contains("://") {
        return reference;
    }
    root + reference
}

fn is_tutorial_catalog(document) {
    if document.type_of() != "map" { return false; }
    if !document.contains("kind") { return false; }
    document["kind"] == TUTORIAL_CATALOG_KIND
}

fn catalog_record(asset_uri, document) {
    let record = #{};
    record.asset_uri = asset_uri;
    record.document = document;
    record
}

fn require_lesson_fields(lesson, entry) {
    for field in ["track", "title", "blurb", "difficulty", "source_asset"] {
        let present = lesson.contains(field);
        let is_string = present && lesson[field].type_of() == "string";
        if !is_string {
            throw(entry + " requires string " + field);
        }
    }
}

fn lesson_scene_asset(lesson, entry) {
    let scene = "";
    if lesson.contains("scene_asset") {
        if lesson["scene_asset"].type_of() != "string" {
            throw(entry + " scene_asset must be a string");
        }
        scene = lesson["scene_asset"];
    }
    scene
}

fn lesson_menu_item(lesson, source_asset, scene_asset) {
    let args = #{};
    args.source_asset = source_asset;
    args.scene_asset = scene_asset;
    // Rhai command reflection uses the typed enum variant spelling.
    args.reload_policy = "Restart";
    let item = #{};
    item.label = lesson["title"] + "  ·  " + lesson["difficulty"];
    item.tooltip = lesson["blurb"];
    item.tool = "tutorials";
    item.hook = "start";
    item.args = args;
    item
}

fn track_menu_item(name, items) {
    let item = #{};
    item.label = name + " (" + items.len().to_string() + ")";
    item.children = items;
    item
}

fn application_asset_lifecycle(event, ctx) {
    if event == "scene_loaded" {
        if ctx.payload.path.type_of() != "string" {
            throw("scene_loaded requires a scene path");
        }
        return empty_menu_contribution();
    }
    if event == "scene_cleared" {
        return empty_menu_contribution();
    }
    if event == "json_scope_loading" {
        if ctx.scope == "twin" && !ctx.payload.has_json_assets {
            return empty_menu_contribution();
        }
        return tutorial_menu_contribution(loading_menu_items(ctx));
    }
    if event != "json_scope_changed" {
        return empty_menu_contribution();
    }

    let catalogs = [];
    let unreadable_assets = "";
    for asset in ctx.payload {
        if asset.error != () {
            if unreadable_assets != "" {
                unreadable_assets += "; ";
            }
            unreadable_assets += asset.asset_uri + ": " + asset.error;
            continue;
        }
        if asset.text == () {
            throw("JSON asset snapshot omitted text for " + asset.asset_uri);
        }
        let document = ();
        let valid_json = true;
        try {
            document = parse_json(asset.text);
        } catch (error) {
            valid_json = false;
        }
        if valid_json && is_tutorial_catalog(document) {
            catalogs.push(catalog_record(asset.asset_uri, document));
        }
    }
    if ctx.scope == "twin" && catalogs.len() == 0 && unreadable_assets == "" {
        return empty_menu_contribution();
    }
    if catalogs.len() == 0 {
        let detail = "no JSON asset is marked " + TUTORIAL_CATALOG_KIND;
        if unreadable_assets != "" {
            detail += "; unreadable JSON assets: " + unreadable_assets;
        }
        throw(detail);
    }
    if catalogs.len() != 1 {
        throw("expected one asset marked " + TUTORIAL_CATALOG_KIND + ", found " + catalogs.len().to_string());
    }

    let catalog_record = catalogs[0];
    let catalog_uri = catalog_record.asset_uri;
    let catalog = catalog_record.document;
    if !catalog.contains("tutorials") || catalog["tutorials"].type_of() != "array" {
        throw(catalog_uri + " must contain a tutorials array");
    }
    if catalog["tutorials"].len() == 0 {
        throw(catalog_uri + " must contain at least one tutorial entry");
    }

    let root = ctx.asset_root_uri;
    let track_names = [];
    let track_items = [];
    let lesson_index = 0;
    for lesson in catalog["tutorials"] {
        let index = lesson_index;
        lesson_index += 1;
        let entry = catalog_uri + ": tutorial catalog entry " + index.to_string();
        if lesson.type_of() != "map" {
            throw(entry + " must be an object");
        }
        require_lesson_fields(lesson, entry);
        let scene_asset = lesson_scene_asset(lesson, entry);

        let track = lesson["track"];
        if blank_text(track) {
            throw(entry + " has an empty track");
        }
        if blank_text(lesson["title"]) {
            throw(entry + " has an empty title");
        }
        if blank_text(lesson["difficulty"]) {
            throw(entry + " has an empty difficulty");
        }
        if blank_text(lesson["source_asset"]) {
            throw(entry + " has an empty source_asset");
        }
        let group = -1;
        for track_index in 0..track_names.len() {
            if track_names[track_index] == track {
                group = track_index;
                break;
            }
        }
        if group < 0 {
            track_names.push(track);
            track_items.push([]);
            group = track_names.len() - 1;
        }
        let source_asset = qualify_catalog_asset(root, lesson["source_asset"]);
        let scene_asset = qualify_catalog_asset(root, scene_asset);
        track_items[group].push(lesson_menu_item(lesson, source_asset, scene_asset));
    }

    let tracks = [];
    for index in 0..track_names.len() {
        tracks.push(track_menu_item(track_names[index], track_items[index]));
    }

    let items = tracks;
    if ctx.scope == "twin" {
        items = [#{ label: ctx.twin_name, children: tracks }];
    }
    #{ menus: [#{ label: "Tutorials", items: items }] }
}
