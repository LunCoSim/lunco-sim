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
    report_verdict(checks, "APPLICATION JSON CATALOGS", "APPLICATION_JSON_CATALOGS");
}
