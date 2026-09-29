// Generic menu action for file-backed authored tutorial scenarios.
fn start(args) {
    cmd("ClearGuidedOverlay", #{});
    let request = #{
        source_asset: args.source_asset,
        scene_asset: args.scene_asset,
        reload_policy: args.reload_policy,
    };
    if args.owner_twin_id != () {
        request.owner_twin_id = args.owner_twin_id;
    }
    cmd("RunScenarioAsset", request)
}
