// Application policy for projecting authored catalog JSON into the generic
// workbench menu contract. Rust supplies text asset lifecycle facts only.
const TUTORIAL_CATALOG_KIND = "lunco.tutorial-catalog.v1";

fn empty_menu_contribution() {
    #{ menus: [] }
}

fn qualify_catalog_asset(root, reference) {
    if reference.trim().is_empty() {
        return "";
    }
    if reference.contains("://") {
        return reference;
    }
    root + reference
}

fn application_asset_lifecycle(event, ctx) {
    if event == "json_scope_loading" {
        if ctx.scope == "twin" && !ctx.payload.has_json_assets {
            return empty_menu_contribution();
        }
        let items = [#{ label: "Loading…", enabled: false }];
        if ctx.scope == "twin" {
            items = [#{ label: ctx.twin_name, children: items }];
        }
        return #{ menus: [#{ label: "Tutorials", items: items }] };
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
        if valid_json && document.type_of() == "map"
            && document.contains("kind")
            && document["kind"] == TUTORIAL_CATALOG_KIND {
            catalogs.push(#{ asset_uri: asset.asset_uri, document: document });
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
        for field in ["track", "title", "blurb", "difficulty", "source_asset"] {
            if !lesson.contains(field) || lesson[field].type_of() != "string" {
                throw(entry + " requires string " + field);
            }
        }
        let scene_asset = "";
        if lesson.contains("scene_asset") {
            if lesson["scene_asset"].type_of() != "string" {
                throw(entry + " scene_asset must be a string");
            }
            scene_asset = lesson["scene_asset"];
        }

        let track = lesson["track"];
        if track.trim().is_empty() {
            throw(entry + " has an empty track");
        }
        if lesson["title"].trim().is_empty() {
            throw(entry + " has an empty title");
        }
        if lesson["difficulty"].trim().is_empty() {
            throw(entry + " has an empty difficulty");
        }
        if lesson["source_asset"].trim().is_empty() {
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
        track_items[group].push(#{
            label: lesson["title"] + "  ·  " + lesson["difficulty"],
            tooltip: lesson["blurb"],
            tool: "tutorials",
            hook: "start",
            args: #{
                source_asset: source_asset,
                scene_asset: scene_asset,
                reload_policy: "restart",
            },
        });
    }

    let tracks = [];
    for index in 0..track_names.len() {
        tracks.push(#{
            label: track_names[index] + " (" + track_items[index].len().to_string() + ")",
            children: track_items[index],
        });
    }

    let items = tracks;
    if ctx.scope == "twin" {
        items = [#{ label: ctx.twin_name, children: tracks }];
    }
    #{ menus: [#{ label: "Tutorials", items: items }] }
}
