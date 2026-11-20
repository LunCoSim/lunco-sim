// Select product-owned resources by their manifest group and key.
fn select_startup_assets(datasets) {
    let startup = #{
        celestial: ["earth", "moon"],
    };
    let selected = [];
    for dataset in datasets {
        if dataset.scope == "engine" && startup.contains(dataset.group)
            && startup[dataset.group].contains(dataset.key) {
            selected.push(dataset.id);
        }
    }
    selected
}
