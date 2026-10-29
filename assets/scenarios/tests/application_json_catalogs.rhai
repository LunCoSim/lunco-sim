// The production asset policy selects authored menus from typed source facts.
fn catalog() {
    `{ "kind":"lunco.tutorial-catalog.v1", "tutorials":[{
      "track":"Probe", "title":"Probe lesson", "blurb":"Catalog source check",
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
        twin_name: "Catalog probe", asset_root_uri: "twin://catalog-probe/",
        payload: records
    }])
}
fn has_lesson(result) {
    result.ok && result.value.menus.len() == 1
        && result.value.menus[0].items[0].children[0].children[0].args.source_asset
            == "twin://catalog-probe/tutorials/probe.rhai"
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
    let first = linked.value.menus[0].items[0].children[0].children[0];
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
    let invalid_twin = cmd("OpenTwinAsset", #{manifest_asset: "not-a-manifest"});
    checks.push(t_true(!invalid_twin.ok && invalid_twin.error.contains("twin.toml"),
        "asset-addressed Twin opening rejects a malformed manifest identity"));
    report_verdict(checks, "APPLICATION JSON CATALOGS", "APPLICATION_JSON_CATALOGS");
}
