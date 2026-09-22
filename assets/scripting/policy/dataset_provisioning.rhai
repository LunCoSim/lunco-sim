// Rust supplies facts about one declared scope. This policy owns the entire
// presentation model and semantic action identifiers; the native layer only
// validates the closed map, publishes it to authored HTML/CSS, and transports
// the resulting action to the Rhai tool layer.
//
// ctx = #{ scope, scope_id, owner, interactive, show_on_start, suppressed,
//          suppressible, datasets: [#{ id, key, name, recommended, processed,
//                                      state }] }
fn dataset_provisioning(ctx) {
    let rows = [];
    let missing = 0;
    let status_labels = #{
        installed: "Ready",
        downloading: "Downloading…",
        processing: "Preparing…",
        cancelling: "Stopping…",
        failed: "Failed",
        cancelled: "Cancelled",
        missing: "Not downloaded",
    };
    for dataset in ctx.datasets {
        let state = dataset.state.kind;
        let needs_download = state == "missing"
            || state == "failed"
            || state == "cancelled";
        if dataset.recommended && needs_download { missing += 1; }
        if !dataset.recommended || !needs_download { continue; }
        rows.push(#{
            id: dataset.id,
            parameter_id: dataset.id,
            label: dataset.name,
            status: status_labels[state],
            action: "request",
            action_label: "Download",
        });
    }

    let visible = ctx.interactive == true
        && ctx.show_on_start == true
        && ctx.suppressed != true
        && missing > 0;
    let scope_id = ctx.scope_id;
    let actions = [
        #{
            key: "request-all",
            label: "Download recommended",
            action: "request_all",
            scope: scope_id,
        },
        #{
            key: "dismiss",
            label: "Not now",
            action: "dismiss",
            scope: scope_id,
        },
    ];
    if ctx.suppressible == true {
        actions.push(#{
            key: "suppress",
            label: "Do not ask again",
            action: "suppress",
            scope: scope_id,
        });
    }
    #{
        visible: visible,
        title: if ctx.scope == "engine" { "Download visual resources" }
            else { "Twin data is not installed" },
        description: if ctx.scope == "engine"
            { "Choose recommended resources for this installation." }
            else { "Choose recommended data for this Twin." },
        scope_label: ctx.owner,
        rows: rows,
        actions: actions,
    }
}
