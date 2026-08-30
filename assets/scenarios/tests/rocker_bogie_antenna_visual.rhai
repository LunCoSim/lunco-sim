// Graphics acceptance for the authored rocker-bogie antenna presentation.
const TEST_KIND = "graphics";
const ROOT = "/RockerBogieAntennaVisual/Rover";
const EPS = 0.001;

fn close_vec(actual, expected) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > EPS {
            return false;
        }
    }
    true
}

fn authored_rotation_ok(path, expected) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["xformOp:rotateXYZ"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    close_vec(record.attrs["xformOp:rotateXYZ"], expected)
}

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;

    // A positive X rotation puts a Z-axis cylinder's front endpoint above its
    // wheel. These authored negative angles are the geometry contract: they
    // bring each forward link's lower endpoint to the wheel hub. The rear links
    // retain their positive slopes and are deliberately not asserted here.
    let links = [
        ["RockerL/FrontLink", -18.43],
        ["RockerR/FrontLink", -18.43],
        ["BogieL/FrontLink", -26.57],
        ["BogieR/FrontLink", -26.57],
    ];
    let failures = [];
    for link in links {
        let path = ROOT + "/" + link[0];
        failures.push(t_true(
            authored_rotation_ok(path, [link[1], 0.0, 0.0]),
            path + " has a detached forward-link pose; expected rotateXYZ=" + link[1]));
    }
    report_verdict(failures, "ROCKER BOGIE ANTENNA VISUAL", "ROCKER_BOGIE_ANTENNA_VISUAL");
}
