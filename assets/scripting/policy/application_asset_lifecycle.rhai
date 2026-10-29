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

fn find_lesson(catalog, root, source) {
    let found = ();
    for candidate in catalog.tutorials {
        let qualified = qualify_catalog_asset(root, candidate.source_asset);
        if qualified != source { continue; }
        if found != () { throw "duplicate tutorial source " + source; }
        found = candidate;
    }
    if found == () { throw "unknown next tutorial source " + source; }
    found
}

fn learning_path(first, catalog, root) {
    let path = [];
    let seen = [];
    let lesson = first;
    loop {
        let source = qualify_catalog_asset(root, lesson.source_asset);
        if seen.contains(source) { throw "tutorial learning path contains a cycle at " + source; }
        seen.push(source);
        let scene = lesson_scene_asset(lesson, source);
        let step = #{};
        step.title = lesson.title;
        step.source_asset = source;
        step.scene_asset = qualify_catalog_asset(root, scene);
        if lesson.contains("twin_asset") { step.twin_asset = qualify_catalog_asset(root, lesson.twin_asset); }
        path.push(step);
        if !lesson.contains("next_source_asset") || lesson.next_source_asset == "" { break; }
        if type_of(lesson.next_source_asset) != "string" { throw source + " next_source_asset must be a string"; }
        let next_source = qualify_catalog_asset(root, lesson.next_source_asset);
        lesson = find_lesson(catalog, root, next_source);
    }
    path
}

fn lesson_menu_item(lesson, source_asset, scene_asset, twin_id, path) {
    let args = #{};
    args.source_asset = source_asset;
    args.scene_asset = scene_asset;
    args.params = #{ lesson_path: path };
    if lesson.contains("twin_asset") {
        if type_of(lesson.twin_asset) != "string" || blank_text(lesson.twin_asset) {
            throw source_asset + " twin_asset must name a manifest asset";
        }
        if scene_asset != "" { throw source_asset + " cannot declare both twin_asset and scene_asset"; }
        args.twin_asset = path[0].twin_asset;
    }
    if twin_id != () {
        args.owner_twin_id = twin_id.parse_int();
    }
    // Rhai command reflection uses the typed enum variant spelling.
    args.reload_policy = "Restart";
    let item = #{};
    item.label = lesson["title"] + "  ·  " + lesson["difficulty"];
    if lesson.contains("lesson_type") {
        if !["tour", "exercise", "challenge"].contains(lesson.lesson_type) {
            throw source_asset + " lesson_type must be tour, exercise, or challenge";
        }
        item.label += "  ·  " + lesson.lesson_type;
    }
    if lesson.contains("duration_minutes") {
        if type_of(lesson.duration_minutes) != "i64" || lesson.duration_minutes < 1 {
            throw source_asset + " duration_minutes must be a positive integer";
        }
        item.label += "  ·  " + lesson.duration_minutes.to_string() + " min";
    }
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
        if asset.persisted_result { continue; }
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
        let path = learning_path(lesson, catalog, root);
        track_items[group].push(lesson_menu_item(lesson, source_asset, scene_asset, ctx.twin_id, path));
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
