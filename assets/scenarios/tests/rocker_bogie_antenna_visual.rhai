// Graphics acceptance for the authored rocker-bogie antenna presentation.
const TEST_KIND = "graphics";
const ROOT = "/RockerBogieAntennaVisual/Rover";
const FEED_ROOT = ROOT + "/Antenna/YawHead/DishGimbal/DishHead";
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

fn authored_translate_ok(path, expected) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["xformOp:translate"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    close_vec(record.attrs["xformOp:translate"], expected)
}

fn authored_feed_pose_ok() {
    let aperture = query("QueryUsdPrim", #{
        path: FEED_ROOT + "/LinkAperture",
        attrs: ["xformOp:translate"],
    });
    let indicator = query("QueryUsdPrim", #{
        path: FEED_ROOT + "/LinkAperture/CommsIndicator",
        attrs: ["xformOp:translate"],
    });
    if aperture == () || aperture.ok == false || aperture.attrs == ()
        || indicator == () || indicator.ok == false || indicator.attrs == () {
        return false;
    }
    close_vec(aperture.attrs["xformOp:translate"], [0.0, 0.36, 0.0])
        && close_vec(indicator.attrs["xformOp:translate"], [0.0, 0.085, 0.0])
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

    // The visual review uses the same component instance as the production
    // rover. These authored transforms are the mount contract: the directional
    // antenna is mounted on the rear deck, and the short-range radio is mounted
    // on the roof rather than floating above it.
    let failures = [];
    failures.push(t_true(
        authored_translate_ok(ROOT + "/Antenna", [-0.35, 0.22, 0.6]),
        "the visual review changed the canonical antenna mount"));
    failures.push(t_true(
        authored_translate_ok(ROOT + "/WifiRadio", [0.0, 0.22, 0.35]),
        "the Wi-Fi radio is not authored on the rover roof"));
    failures.push(t_true(
        authored_feed_pose_ok(),
        "the shared feed indicator is not centered on the LinkAperture bore axis"));
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
    for link in links {
        let path = ROOT + "/" + link[0];
        failures.push(t_true(
            authored_rotation_ok(path, [link[1], 0.0, 0.0]),
            path + " has a detached forward-link pose; expected rotateXYZ=" + link[1]));
    }
    report_verdict(failures, "ROCKER BOGIE ANTENNA VISUAL", "ROCKER_BOGIE_ANTENNA_VISUAL");
}
