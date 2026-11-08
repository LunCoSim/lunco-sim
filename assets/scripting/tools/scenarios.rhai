// Typed operations referenced by the Rhai-authored Scenarios menu.
fn load(args) {
    if args.open_twin == true {
        return cmd("OpenTwinScene", #{
            twin_id: args.twin_id,
            name: args.authority,
            relative_path: args.relative_path
        });
    }
    cmd("LoadScene", #{ path: args.uri, root_prim: "" })
}

fn restart(_args) {
    cmd("RestartScene", #{});
}

fn toggle_test_scenes(args) {
    cmd("SetShowTestAssets", #{ show: !args.show_test_assets });
}

fn open_downloaded(args) {
    let mounted = cmd("MountCachedScenarioTwin", #{
        scenario_id: args.scenario_id,
        revision: args.revision,
        name: args.name,
        scene: args.scene
    });
    if mounted == () || !mounted.ok { throw "downloaded scenario could not be mounted"; }
    cmd("LoadScene", #{ path: mounted.value.path, root_prim: "" });
}
