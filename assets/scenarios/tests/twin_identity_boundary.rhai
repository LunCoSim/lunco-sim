// Production Twin-scoped API boundary regression. Workspace Twin IDs remain
// available to application selectors; Twin-authored execution receives no ID.
const TEST_KIND = "headless";

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [], entity_reads: [], entity_writes: [],
       query_reads: ["ReadActiveTwinContract", "ScriptingCatalog", "ScriptComplete"], required_inputs: [] }
}

fn workspace_selector(name, type_name) {
    ["twin_id", "owner_twin_id", "target_twin_id", "workspace_twin_id",
        "workspace_twin", "active_twin_id", "scope_id", "active_twin", "host_twin"].contains(name)
        || (name == "twin" && (type_name.contains("u64") || type_name.contains("TwinId")))
}

fn on_start(me, ctx) {
    let runtime = execution_context();
    let lifecycle = policy_status().lifecycle.runtime_context;
    let contract = query("ReadActiveTwinContract", #{});
    let catalog = query("ScriptingCatalog", #{sections: ["commands", "queries"]});
    let command_completion = query("ScriptComplete", #{prefix: "OpenTwinScene"});
    let query_completion = query("ScriptComplete", #{prefix: "ReadActiveTwinContract"});
    let forged_twin_target = cmd("OpenTwinScene", #{
        twin_id: 999999999,
        relative_path: "replacement.usda",
    });

    let command_schema_hidden = catalog != () && catalog.commands != ();
    if command_schema_hidden {
        for command in catalog.commands {
            for field in command.fields {
                if workspace_selector(field.name, field.type_name) {
                    command_schema_hidden = false;
                }
            }
        }
    }

    let query_schema_hidden = catalog != () && catalog.queries != ();
    if query_schema_hidden {
        for query_schema in catalog.queries {
            if query_schema.parameters != () {
                for parameter in query_schema.parameters {
                    if workspace_selector(parameter.name, parameter.type_name) {
                        query_schema_hidden = false;
                    }
                }
            }
        }
    }

    let completion_schema_hidden = command_completion.candidates.len() > 0
        && query_completion.candidates.len() > 0;
    for candidate in command_completion.candidates {
        if candidate.label == "OpenTwinScene" {
            for name in ["twin_id", "owner_twin_id", "target_twin_id", "workspace_twin_id",
                    "workspace_twin", "active_twin_id", "scope_id", "active_twin", "host_twin", "twin"] {
                if candidate.detail.contains(name) { completion_schema_hidden = false; }
            }
        }
    }
    for candidate in query_completion.candidates {
        if candidate.label == "ReadActiveTwinContract" && candidate.schema != () {
            let schema = candidate.schema;
            if schema.parameters != () {
                for parameter in schema.parameters {
                    if workspace_selector(parameter.name, parameter.type_name) {
                        completion_schema_hidden = false;
                    }
                }
            }
            if schema.response != () { completion_schema_hidden = false; }
        }
    }

    report_verdict([
        t_true(runtime.scope == "twin" && !runtime.contains("owner_id")
            && !runtime.contains("target_twin_id") && !runtime.contains("twin_id"),
            "Twin execution context omits application-owned Twin selectors"),
        t_true(!lifecycle.contains("owner_id") && !lifecycle.contains("target_twin_id")
            && !lifecycle.contains("twin_id"),
            "Twin lifecycle status omits application-owned Twin selectors"),
        t_true(contract != () && !contract.contains("twin_id"),
            "Twin-scoped contract queries omit the workspace Twin ID"),
        t_true(!forged_twin_target.ok && forged_twin_target.status == "rejected"
            && forged_twin_target.error.contains("cannot supply application-owned Twin identity"),
            "Twin-scoped commands reject caller-supplied workspace Twin IDs"),
        t_true(command_schema_hidden,
            "Twin-scoped command discovery omits workspace Twin selectors"),
        t_true(query_schema_hidden,
            "Twin-scoped query discovery omits workspace Twin selectors"),
        t_true(completion_schema_hidden,
            "Twin-scoped completion omits workspace selectors and query response details"),
    ], "Twin execution identity boundary", "TWIN_IDENTITY_BOUNDARY");
}
