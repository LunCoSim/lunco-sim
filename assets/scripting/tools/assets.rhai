// assets.rhai — high-level dataset provisioning policy.
//
// Rust owns the transport, archive extraction, decoding, raster math,
// cancellation, and atomic installation. This library owns only selection and
// sequencing policy, so a Twin can choose what to provision without a Rust
// rebuild or a new one-off command.

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
