// Generic menu action for file-backed authored tutorial scenarios.
fn start(args) {
    cmd("RunScenarioAsset", #{
        source_asset: args.source_asset,
        scene_asset: args.scene_asset,
        reload_policy: args.reload_policy,
    })
}
