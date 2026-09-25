//! Shipped celestial dataset declarations are visible through the production
//! dataset query surface; no Rust test reads the asset library directly.

const TEST_KIND = "headless";
const WAIT_S = 8.0;

fn metadata_body_id(dataset) {
    if dataset == () || dataset.metadata == () || dataset.metadata.body == () {
        return -1;
    }
    dataset.metadata.body.naif_id
}

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();
    let response = query("ListDatasets", #{});
    if response == () || response.datasets == () {
        if this.elapsed < WAIT_S { return; }
        this.done = true;
        report_verdict([t_true(false, "ListDatasets did not publish its dataset registry")],
            "CELESTIAL DATASET CONTRACT", "CELESTIAL_DATASET_CONTRACT");
        return;
    }

    let earth = ();
    let moon = ();
    for dataset in response.datasets {
        if dataset.group == "celestial" && dataset.key == "earth" { earth = dataset; }
        if dataset.group == "celestial" && dataset.key == "moon" { moon = dataset; }
    }
    if earth == () || moon == () {
        if this.elapsed < WAIT_S { return; }
    }

    this.done = true;
    let checks = [];
    checks.push(t_true(earth != () && metadata_body_id(earth) == 399,
        "earth imagery must declare NAIF 399 in its dataset metadata"));
    checks.push(t_true(moon != () && metadata_body_id(moon) == 301,
        "moon imagery must declare NAIF 301 in its dataset metadata"));
    checks.push(t_true(earth != () && earth.processed == true
        && earth.artifact_uri == "lunco://textures/earth.png",
        "earth must expose its processed logical artifact URI"));
    checks.push(t_true(moon != () && moon.processed == true
        && moon.artifact_uri == "lunco://textures/moon.png",
        "moon must expose its processed logical artifact URI"));
    report_verdict(checks, "CELESTIAL DATASET CONTRACT", "CELESTIAL_DATASET_CONTRACT");
}
