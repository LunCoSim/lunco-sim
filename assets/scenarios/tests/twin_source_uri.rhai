// Production acceptance for stable Twin source addresses and internal mount
// lease containment. The API driver supplies current application observations.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ReadActiveTwinContract", "InspectWorkbenchNavigation",
           "ScriptingCatalog", "ScriptComplete"], required_inputs: [] }
}

fn on_start(me, params) {
    let mode = params.mode;
    if mode == "source" {
        let catalog = query("ScriptingCatalog", #{sections: ["commands"]});
        let completions = query("ScriptComplete", #{prefix: "twin_"});
        let old_getter = false;
        let stable_getter = false;
        for entry in completions.candidates {
            if entry.label == "twin_authority" { old_getter = true; }
            if entry.label == "twin_source_identity" && entry.kind == "verb" {
                stable_getter = true;
            }
        }
        let expected = ["OpenTwinScene", "LoadTwinToolLibrary", "LoadTwinTimelineFile",
            "LoadTwinSysmlSource", "PrepareTwinSysmlAnalysis", "LoadTwinModelicaSourceRoot"];
        let found = [];
        let clean = true;
        for command in catalog.commands {
            if expected.contains(command.name) {
                found.push(command.name);
                for field in command.fields {
                    if field.name == "authority" { clean = false; }
                }
            }
        }
        let twin = query("ReadActiveTwinContract", #{});
        let navigation = query("InspectWorkbenchNavigation", #{});
        report_verdict([
            t_true(!old_getter && stable_getter,
                "Rhai exposes stable Twin source identity without the mount lease getter"),
            t_true(found.len() == expected.len() && clean,
                "Twin commands omit mount lease fields"),
            t_true(twin.open && twin.name == params.name,
                "the authored Twin display name is unchanged"),
            t_true(twin_display_name() == params.name,
                "the scripting display name is unchanged"),
            t_true(twin_source_identity() == params.identity,
                "scene operations preserve the stable Twin source identity"),
            t_true(params.uri.starts_with("twin://" + params.identity + "/"),
                "the source URI is rooted in the stable Twin identity"),
            t_true(navigation.scene_path == params.uri,
                "the visible scene URI remains stable"),
        ], "Twin source URI", "TWIN_SOURCE_URI");
        return;
    }

    if mode == "progress" {
        let safe = params.reasons.len() > 0;
        for reason in params.reasons {
            if reason.contains("twin://") { safe = false; }
        }
        let stable = params.labels.len() > 0;
        let expected = false;
        for label in params.labels {
            if label == params.uri { expected = true; }
            if label == "deferred prims" || label == "Twin scene" { continue; }
            if !label.starts_with(params.source_prefix) { stable = false; }
        }
        report_verdict([
            t_true(safe, "scene progress reasons do not expose a Twin mount URI"),
            t_true(expected, "pending scene readiness includes the stable URI"),
            t_true(stable, "pending Twin labels use stable source identities or generic text"),
        ], "Twin scene progress", "TWIN_SOURCE_URI_PROGRESS");
        return;
    }

    if mode == "reject" {
        let unknown = cmd("LoadScene", #{
            path: "twin://unmounted-source-negative/scene.usda", root_prim: ""
        });
        let invalid = cmd("LoadScene", #{
            path: "twin://invalid-mount-lease-negative/" + params.relative_path,
            root_prim: ""
        });
        let navigation = query("InspectWorkbenchNavigation", #{});
        report_verdict([
            t_true(!unknown.ok && unknown.status != "pending",
                "an unmounted source is terminally rejected"),
            t_true(!invalid.ok && invalid.status != "pending",
                "an opaque mount-like identifier is not a public source identity"),
            t_true(navigation.scene_path == params.uri,
                "rejected addresses preserve the admitted stable scene URI"),
        ], "Twin source URI rejection", "TWIN_SOURCE_URI_NEGATIVE");
        return;
    }

    report_verdict([t_true(false, "the API driver supplied a known Twin source test mode")],
        "Twin source URI", "TWIN_SOURCE_URI");
}
