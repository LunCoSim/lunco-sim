// assets.rhai — high-level dataset provisioning policy.
//
// Rust owns the transport, archive extraction, decoding, raster math,
// cancellation, and atomic publication. This library selects declared
// pipelines and sequences their commands, so a Twin can bake manifest entries
// without a per-dataset Rust command or rebuild.

fn list(scope) {
    let params = if scope == () || scope == "" { #{} } else { #{ scope: scope } };
    let result = query("ListDatasets", params);
    if result == () {
        return #{ ok: false, error: "dataset query returned no result" };
    }
    result
}

fn find(id) {
    let result = list("");
    if result.ok == false { return result; }
    for dataset in result.datasets {
        if dataset.id == id { return #{ ok: true, dataset: dataset }; }
    }
    #{ ok: false, error: "dataset is not declared: " + id }
}

fn request(id) {
    let declared = find(id);
    if declared.ok == false { return declared; }
    if declared.dataset.state.kind == "installed" {
        return #{ ok: true, status: "already_installed", id: id };
    }
    cmd("RequestDataset", #{ id: id })
}

/// Process a declared dataset source through its `Assets.toml` pipeline.
/// The Rust worker reads the source and processor config from the manifest;
/// the command carries only the dataset id. Matching bake keys are skipped.
fn bake(id) {
    let declared = find(id);
    if declared.ok == false { return declared; }
    if declared.dataset.processed == false {
        return #{ ok: false, error: "dataset has no manifest processing pipeline: " + id };
    }
    let kind = declared.dataset.state.kind;
    if kind == "downloading" || kind == "processing" || kind == "cancelling" {
        return #{ ok: false, error: "dataset already has an active operation: " + id };
    }
    cmd("ProcessDataset", #{ id: id })
}

/// Queue every idle manifest pipeline in a scope. Each worker resolves its
/// source and authored config independently through the dataset registry.
fn bake_scope(scope) {
    let result = list(scope);
    if result.ok == false { return result; }
    let bakes = [];
    for dataset in result.datasets {
        let kind = dataset.state.kind;
        if dataset.processed && kind != "downloading" && kind != "processing" && kind != "cancelling" {
            bakes.push(bake(dataset.id));
        }
    }
    #{ ok: true, bakes: bakes }
}

fn cancel(id) {
    let declared = find(id);
    if declared.ok == false { return declared; }
    cmd("CancelDataset", #{ id: id })
}

/// Request every declared recommended dataset in a scope. The worker then
/// executes each manifest's authored download/process declaration off-thread.
fn request_recommended(scope) {
    let result = list(scope);
    if result.ok == false { return result; }
    let requests = [];
    for dataset in result.datasets {
        if dataset.recommended && dataset.state.kind != "installed" {
            requests.push(request(dataset.id));
        }
    }
    #{ ok: true, requested: requests }
}
