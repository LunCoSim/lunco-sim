// assets.provision receives one scope's authoritative dataset state. Rhai owns
// visibility, wording, and actions; the host validates and publishes this model.
fn dataset_download_status(state) {
    let done = state.bytes_done.to_float();
    let total = state.bytes_total.to_float();
    let mib = ((done / 1048576.0) * 10.0).round() / 10.0;
    if total <= 0.0 {
        return "Downloading · " + mib.to_string() + " MiB received";
    }
    let percentage = done / total * 100.0;
    let percent = percentage.floor().to_int();
    let total_mib = ((total / 1048576.0) * 10.0).round() / 10.0;
    let status = "Downloading · " + percent.to_string() + "% · ";
    status += mib.to_string() + " / " + total_mib.to_string() + " MiB";
    status
}

fn dataset_provisioning(ctx) {
    let rows = [];
    let missing = 0;
    let busy = 0;
    let recommended = 0;
    for dataset in ctx.datasets {
        if !dataset.recommended { continue; }
        recommended += 1;
        let state = dataset.state.kind;
        if state == "installed" { continue; }
        let status = "Not downloaded";
        let action = "request";
        let action_label = "Download";
        if state == "downloading" {
            busy += 1;
            action = "cancel";
            action_label = "Cancel";
            status = dataset_download_status(dataset.state);
        }
        if state == "processing" {
            busy += 1;
            status = "Preparing · " + dataset.state.detail;
            action = "cancel";
            action_label = "Cancel";
        }
        if state == "cancelling" {
            busy += 1;
            status = "Stopping…";
            action = "dismiss";
            action_label = "Hide";
        }
        if state == "missing" || state == "failed" || state == "cancelled" {
            missing += 1;
        }
        if state == "failed" {
            status = "Failed · " + dataset.state.detail;
            action_label = "Retry";
        }
        if state == "cancelled" {
            status = "Cancelled";
            action_label = "Retry";
        }
        if !["missing", "downloading", "processing", "cancelling", "failed", "cancelled"].contains(state) {
            throw "Unknown dataset state: " + state;
        }
        rows.push(#{
            id: dataset.id, parameter_id: dataset.id, scope: ctx.scope_id,
            label: dataset.name, status: status, action: action, action_label: action_label,
        });
    }

    let complete = missing == 0 && busy == 0;
    let actions = [];
    if missing > 0 {
        actions.push(#{key: "request-all", label: "Download recommended",
            action: "request_all", scope: ctx.scope_id});
    }
    actions.push(#{key: "dismiss",
        label: if busy > 0 { "Continue in background" } else if complete { "Close" } else { "Not now" },
        action: "dismiss", scope: ctx.scope_id});
    if ctx.suppressible == true && busy == 0 && !complete {
        actions.push(#{key: "suppress", label: "Do not ask again",
            action: "suppress", scope: ctx.scope_id});
    }
    #{
        visible: ctx.interactive && ctx.show_on_start && !ctx.suppressed && recommended > 0,
        title: if complete { "Data ready" } else if busy > 0 { "Preparing data" }
            else if ctx.scope == "engine" { "Download visual resources" } else { "Twin data is not installed" },
        description: if complete {
            "Selected datasets are installed. Scene readiness and remaining errors are shown in Recent Events."
        } else if ctx.scope == "engine" {
            "Choose resources for this installation. Progress is also available in Downloadable data."
        } else {
            "Required terrain must be ready before simulation can run. Downloads and preparation continue in the background; progress remains in Twin resources."
        },
        scope_label: ctx.owner, rows: rows, actions: actions,
    }
}
