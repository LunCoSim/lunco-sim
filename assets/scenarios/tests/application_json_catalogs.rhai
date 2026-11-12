// The production asset policy selects authored menus from typed source facts.
fn catalog() {
    `{ "kind":"lunco.tutorial-catalog.v1", "tutorials":[{
      "track":"Probe", "title":"Probe lesson", "featured":true,
      "featured_description":"Quick action summary",
      "blurb":"Catalog source check",
      "difficulty":"beginner", "source_asset":"tutorials/probe.rhai"
    }] }`
}
fn record(path, persisted_result, text, error) {
    #{asset_uri: "twin://catalog-probe/" + path,
      persisted_result: persisted_result, text: text, error: error}
}
fn project(records) {
    invoke_hook("application.asset.lifecycle", ["json_scope_changed", #{
        provider: "catalog-source-probe", scope: "twin", twin_id: "42",
        twin_display_name: "Catalog probe", asset_root_uri: "twin://catalog-probe/",
        payload: records
    }])
}
fn has_lesson(result) {
    result.ok && result.value.menus.len() == 1
        && result.value.menus[0].label == "Scenarios"
        && result.value.menus[0].order == -50
        && !result.value.menus[0].separator_before
        && result.value.menus[0].items[0].label == "Catalog probe"
        && result.value.menus[0].items[0].children[0].label == "Tutorials"
        && result.value.menus[0].items[0].children[0].children[0].children[0].args.source_asset
            == "twin://catalog-probe/tutorials/probe.rhai"
}
fn application_project(records) {
    invoke_hook("application.asset.lifecycle", ["json_scope_changed", #{
        provider: "catalog-application-probe", scope: "application", twin_id: (),
        twin_display_name: (), asset_root_uri: "twin://catalog-probe/", payload: records
    }])
}
fn scenario_test_scene(uri, relative_path, stem, is_test, twin) {
    #{ uri: uri, relative_path: relative_path, stem: stem,
        description: (), is_test: is_test, twin: twin }
}
fn scenario_find_label(items, label) {
    for item in items {
        if item.label == label { return item; }
        if item.contains("children") {
            let found = scenario_find_label(item.children, label);
            if found != () { return found; }
        }
    }
    ()
}
fn scenario_menu_items(result) {
    let items = [];
    for menu in result.menus {
        for item in menu.items { items.push(item); }
    }
    items
}
fn viewport_has_label(actions, label) {
    for action in actions { if action.label == label { return true; } }
    false
}
fn scenario_menu_facts(show_test_assets) {
    let twin = #{ id: "42", source_identity: "station", source_ready: true, name: "Station",
        directory: "station-folder",
        default_scene: "scenes/main.usda" };
    let scenes = [
        scenario_test_scene("lunco://scenes/demo.usda", "demo.usda", "demo_scene", false, ()),
        scenario_test_scene("lunco://scenes/tests/probe.usda", "tests/probe.usda", "probe_test", true, ()),
        scenario_test_scene("twin://station/scenes/main.usda", "scenes/main.usda", "main_scene", false, twin),
        scenario_test_scene("twin://station/scenes/tests/test.usda", "scenes/tests/test.usda", "test_scene", true, twin)
    ];
    #{ current_scene: "twin://station/scenes/main.usda", active_twin: twin,
        twins: [twin], cached_twins: [], download_sync_enabled: false,
        show_test_assets: show_test_assets,
        catalog_ready: true, catalog_error: (), scenes: scenes }
}
fn linked_catalog(next) {
    `{ "kind":"lunco.tutorial-catalog.v1", "tutorials":[{
      "track":"Start Here", "title":"Drive", "blurb":"Drive the training rover",
      "difficulty":"beginner", "lesson_type":"exercise", "duration_minutes":5,
      "source_asset":"drive.rhai", "scene_asset":"yard.usda", "next_source_asset":"`
      + next + `" }, {
      "track":"Start Here", "title":"Measure", "blurb":"Read a measured signal",
      "difficulty":"beginner", "lesson_type":"exercise", "duration_minutes":3,
      "source_asset":"measure.rhai", "scene_asset":"yard.usda"
    }] }`
}
fn on_start(me, ctx) {
    let source = record("arbitrary-catalog.json", false, catalog(), ());
    let checks = [];
    checks.push(t_true(has_lesson(project([source])),
        "an authored catalog retains its qualified lesson command"));
    let twin_featured = project([source]);
    let twin_action = ();
    if twin_featured.ok && twin_featured.value.viewport_actions.len() == 1 {
        twin_action = twin_featured.value.viewport_actions[0];
    }
    checks.push(t_true(twin_action != () && !twin_action.args.contains("owner_twin_id")
        && twin_action.args.source_asset == "twin://catalog-probe/tutorials/probe.rhai"
        && twin_action.visibility == "always",
        "a Twin featured action uses its bound owner and qualified lesson without a Twin ID payload"));
    checks.push(t_true(has_lesson(project([
        source, record("results/snapshot.json", true, catalog(), ())
    ])), "persisted results cannot add a second tutorial catalog"));
    let only_result = project([record("results/snapshot.json", true, catalog(), ())]);
    checks.push(t_true(only_result.ok && only_result.value.menus.len() == 0,
        "a result with a catalog marker does not create menus"));
    let unreadable_result = project([record("results/unreadable.json", true, (), "read rejected")]);
    checks.push(t_true(unreadable_result.ok && unreadable_result.value.menus.len() == 0,
        "result read failures remain with their artifact owner"));
    checks.push(t_true(has_lesson(project([record("results-copy/catalog.json", false, catalog(), ())])),
        "authored sources with similar directory names remain eligible"));
    let malformed = project([record("catalog.json", false,
        `{ "kind":"lunco.tutorial-catalog.v1", "tutorials":[{}] }`, ())]);
    checks.push(t_true(!malformed.ok && malformed.error.contains("requires string track"),
        "malformed authored catalogs fail visibly"));
    let linked = project([record("catalog.json", false, linked_catalog("measure.rhai"), ())]);
    let first = linked.value.menus[0].items[0].children[0].children[0].children[0];
    let path = first.args.params.lesson_path;
    checks.push(t_true(linked.ok && first.label.contains("exercise") && first.label.contains("5 min")
        && path.len() == 2 && path[1].source_asset == "twin://catalog-probe/measure.rhai"
        && path[1].scene_asset == "twin://catalog-probe/yard.usda",
        "catalog metadata and the complete next-lesson path reach the generic launcher"));
    let missing_next = project([record("catalog.json", false, linked_catalog("missing.rhai"), ())]);
    checks.push(t_true(!missing_next.ok && missing_next.error.contains("unknown next tutorial"),
        "an unresolved continuation is rejected visibly"));
    let cycle = project([record("catalog.json", false, linked_catalog("drive.rhai"), ())]);
    checks.push(t_true(!cycle.ok && cycle.error.contains("cycle"),
        "a cyclic learning path cannot produce an endless continuation"));
    let featured = application_project([record("featured.json", false, catalog(), ())]);
    let quick_action = ();
    if featured.ok && featured.value.viewport_actions.len() == 1 {
        quick_action = featured.value.viewport_actions[0];
    }
    checks.push(t_true(quick_action != ()
        && quick_action.label == "Start Probe lesson" && quick_action.primary
        && quick_action.description == "Quick action summary"
        && quick_action.tool == "tutorials" && quick_action.hook == "start"
        && quick_action.visibility == "no_active_twin"
        && featured.value.menus[0].order == 0
        && featured.value.menus[0].separator_before
        && quick_action.args.source_asset == "twin://catalog-probe/tutorials/probe.rhai",
        "the explicitly featured catalog entry also supplies a generic welcome action"));
    let multiple_featured = application_project([record("featured.json", false,
        `{ "kind":"lunco.tutorial-catalog.v1", "tutorials":[{
          "track":"Probe", "title":"One", "featured":true, "blurb":"One",
          "difficulty":"beginner", "source_asset":"one.rhai" }, {
          "track":"Probe", "title":"Two", "featured":true, "blurb":"Two",
          "difficulty":"beginner", "source_asset":"two.rhai" }] }`, ())]);
    checks.push(t_true(!multiple_featured.ok && multiple_featured.error.contains("only one lesson featured"),
        "ambiguous welcome actions fail at the authored policy owner"));
    let invalid_twin = cmd("OpenTwinAsset", #{manifest_asset: "not-a-manifest"});
    checks.push(t_true(!invalid_twin.ok && invalid_twin.error.contains("twin.toml"),
        "asset-addressed Twin opening rejects a malformed manifest identity"));

    let hidden = invoke_hook("application.scenarios.menu", [scenario_menu_facts(false)]);
    if !hidden.ok { throw hidden.error; }
    let hidden_menu = hidden.value.menus[0];
    let hidden_items = scenario_menu_items(hidden.value);
    let sim_core_settings = scenario_find_label(hidden.value.settings_submenus, "Sim Core");
    let tests_toggle = scenario_find_label(sim_core_settings.items, "Show library test scenes");
    let restart = scenario_find_label(hidden_menu.items, "Restart Scenario");
    let current = scenario_find_label(hidden_menu.items, "Main Scene");
    checks.push(t_true(hidden_menu.label == "Scenarios"
        && sim_core_settings != () && tests_toggle != ()
        && tests_toggle.selection == "checkbox" && !tests_toggle.selected
        && tests_toggle.hook == "toggle_test_scenes"
        && scenario_find_label(hidden_items, "Show library test scenes") == (),
        "Rhai contributes test-scene visibility under Settings / Sim Core"));
    checks.push(t_true(restart != () && restart.tool == "scenarios"
        && restart.hook == "restart" && restart.enabled,
        "the Rhai Scenarios menu enables restart while a scene is loaded"));
    checks.push(t_true(current != () && current.selection == "radio" && current.selected
        && current.args.uri == "twin://station/scenes/main.usda",
        "the committed scene is selected by the Rhai policy"));
    let station_group = scenario_find_label(hidden_items, "Station");
    let station_has_scenes = false;
    let station_has_tests = false;
    let station_default = ();
    if station_group != () {
        station_default = scenario_find_label(station_group.children, "Load Default Scene");
        station_has_scenes = scenario_find_label(station_group.children, "Main Scene") != ();
        let station_tests = scenario_find_label(station_group.children, "Test Scenes");
        station_has_tests = station_tests != ()
            && scenario_find_label(station_tests.children, "Test Scene") != ();
    }
    checks.push(t_true(station_default != () && station_default.hook == "load"
        && station_default.args.open_twin
        && station_default.args.relative_path == "scenes/main.usda"
        && station_has_scenes && station_has_tests,
        "Twin menus offer the default scene, regular scenes, and a separate Test Scenes submenu"));
    let has_divider = false;
    for item in hidden_items { if item.contains("separator") && item.separator { has_divider = true; } }
    checks.push(t_true(has_divider,
        "the Scenarios policy restores authored section dividers"));
    checks.push(t_true(scenario_find_label(hidden_items, "Probe Test") == (),
        "hidden library test scenes are filtered"));
    checks.push(t_true(hidden.value.menus.len() == 2
        && hidden.value.menus[0].order < hidden.value.menus[1].order
        && scenario_find_label(hidden.value.menus[0].items, "Restart Scenario") != ()
        && scenario_find_label(hidden.value.menus[1].items, "Library") != (),
        "Restart and Twin menus precede the later Library contribution"));
    checks.push(t_true(hidden.value.viewport_guidance == "Choose a scene from Station."
        && hidden.value.viewport_actions.len() == 1
        && hidden.value.viewport_actions[0].label == "Load default scene"
        && !viewport_has_label(hidden.value.viewport_actions, "Test Scene")
        && hidden.value.viewport_actions[0].args.twin_id == 42
        && !hidden.value.viewport_actions[0].args.contains("authority")
        && hidden.value.viewport_actions[0].args.relative_path == "scenes/main.usda",
        "the active-Twin welcome action comes from the Rhai policy and excludes test scenes"));
    checks.push(t_true(hidden.value.navigation_entries.len() == 3,
        "quick-open destinations use the same policy visibility rules"));

    let visible = invoke_hook("application.scenarios.menu", [scenario_menu_facts(true)]);
    if !visible.ok { throw visible.error; }
    let visible_toggle = scenario_find_label(
        scenario_find_label(visible.value.settings_submenus, "Sim Core").items,
        "Show library test scenes");
    checks.push(t_true(scenario_find_label(scenario_menu_items(visible.value), "Probe Test") != ()
        && visible_toggle != () && visible_toggle.selected,
        "the Sim Core setting shows library tests in Scenarios"));
    checks.push(t_true(visible.value.navigation_entries.len() == 4,
        "showing library tests adds them to quick-open"));

    let fallback_facts = scenario_menu_facts(false);
    let fallback_twin = #{ id: "43", source_identity: "fallback", source_ready: true, name: " ",
        directory: "fallback-folder", default_scene: "scenes/main.usda" };
    fallback_facts.active_twin = fallback_twin;
    fallback_facts.twins = [fallback_twin];
    fallback_facts.scenes[2].twin = fallback_twin;
    fallback_facts.scenes[3].twin = fallback_twin;
    let fallback = invoke_hook("application.scenarios.menu", [fallback_facts]);
    if !fallback.ok { throw fallback.error; }
    checks.push(t_true(fallback.value.viewport_guidance == "Choose a scene from fallback-folder."
        && scenario_find_label(scenario_menu_items(fallback.value), "fallback-folder") != (),
        "Twin display labels fall back to their source directory in Rhai"));
    let id_fallback_facts = scenario_menu_facts(false);
    let id_fallback_twin = #{ id: "44", source_identity: "fallback", source_ready: true, name: "", directory: (),
        default_scene: "scenes/main.usda" };
    id_fallback_facts.active_twin = id_fallback_twin;
    id_fallback_facts.twins = [id_fallback_twin];
    id_fallback_facts.scenes[2].twin = id_fallback_twin;
    id_fallback_facts.scenes[3].twin = id_fallback_twin;
    let id_fallback = invoke_hook("application.scenarios.menu", [id_fallback_facts]);
    if !id_fallback.ok { throw id_fallback.error; }
    checks.push(t_true(id_fallback.value.viewport_guidance == "Choose a scene from Project 44."
        && scenario_find_label(scenario_menu_items(id_fallback.value), "Project 44") != (),
        "Twin display labels fall back to their stable identity in Rhai"));

    let start_facts = scenario_menu_facts(false);
    start_facts.active_twin = ();
    start_facts.current_scene = ();
    let start_menu = invoke_hook("application.scenarios.menu", [start_facts]);
    if !start_menu.ok { throw start_menu.error; }
    checks.push(t_true(start_menu.value.viewport_actions.len() == 1
        && start_menu.value.viewport_actions[0].label == "Explore demo sandbox"
        && start_menu.value.viewport_actions[0].tool == "scenarios"
        && start_menu.value.viewport_actions[0].args.uri == "lunco://scenes/luncosim/sandbox_scene.usda",
        "the demo welcome action is authored by the Scenarios policy"));
    let start_restart = scenario_find_label(scenario_menu_items(start_menu.value), "Restart Scenario");
    checks.push(t_true(start_restart != () && start_restart.tool == "scenarios"
        && start_restart.hook == "restart" && !start_restart.enabled,
        "Restart Scenario stays at the top but is disabled while no scene is loaded"));

    let many_scenes = scenario_menu_facts(false);
    for index in 0..20 {
        many_scenes.scenes.push(scenario_test_scene(
            "twin://station/scenes/extra_" + index.to_string() + ".usda",
            "scenes/extra_" + index.to_string() + ".usda",
            "extra_" + index.to_string(), false, many_scenes.active_twin));
    }
    let many = invoke_hook("application.scenarios.menu", [many_scenes]);
    if !many.ok { throw many.error; }
    checks.push(t_true(many.value.viewport_actions.len() == 21
        && many.value.viewport_actions[0].label == "Load default scene"
        && viewport_has_label(many.value.viewport_actions, "Extra 0")
        && viewport_has_label(many.value.viewport_actions, "Extra 19")
        && !viewport_has_label(many.value.viewport_actions, "Test Scene"),
        "the Twin welcome list contains every regular scene and excludes its test scenes"));
    report_verdict(checks, "APPLICATION JSON CATALOGS", "APPLICATION_JSON_CATALOGS");
}
