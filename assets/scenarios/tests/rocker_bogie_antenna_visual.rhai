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

fn close_scalar(actual, expected) {
    actual != () && (actual - expected).abs() <= EPS
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

fn authored_cylinder_ok(path, expected_axis, expected_radius, expected_height, expected_translate) {
    let record = query("QueryUsdPrim", #{
        path: path,
        attrs: ["axis", "radius", "height", "xformOp:translate"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    record.attrs["axis"] == expected_axis
        && close_scalar(record.attrs["radius"], expected_radius)
        && close_scalar(record.attrs["height"], expected_height)
        && close_vec(record.attrs["xformOp:translate"], expected_translate)
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

fn wheel_shader_contract_ok(path) {
    let record = query("QueryUsdPrim", #{
        path: path + "/TireLook/Shader",
        attrs: ["info:wgsl:sourceAsset", "inputs:dust_amount", "inputs:dust_color"],
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    record.attrs["info:wgsl:sourceAsset"] == "lunco://shaders/wheel.wgsl"
        && record.attrs["inputs:dust_amount"] == ()
        && record.attrs["inputs:dust_color"] == ()
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
    // Every link and axle is checked as a composed USD cylinder. The endpoint
    // contract is represented by the shared link/axle translations, lengths,
    // and axis declarations; checking all eight links catches either side's
    // front and rear assembly rather than only the previously visible links.
    let links = [
        ["RockerL/FrontLink", -18.43, 0.06, 1.265, [0.0, -0.2, -0.6]],
        ["RockerL/RearLink", 18.43, 0.06, 0.632, [0.0, -0.1, 0.3]],
        ["RockerR/FrontLink", -18.43, 0.06, 1.265, [0.0, -0.2, -0.6]],
        ["RockerR/RearLink", 18.43, 0.06, 0.632, [0.0, -0.1, 0.3]],
        ["BogieL/FrontLink", -26.57, 0.05, 0.447, [0.0, -0.1, -0.2]],
        ["BogieL/RearLink", 18.43, 0.05, 0.632, [0.0, -0.1, 0.3]],
        ["BogieR/FrontLink", -26.57, 0.05, 0.447, [0.0, -0.1, -0.2]],
        ["BogieR/RearLink", 18.43, 0.05, 0.632, [0.0, -0.1, 0.3]],
    ];
    for link in links {
        let path = ROOT + "/" + link[0];
        failures.push(t_true(
            authored_cylinder_ok(path, "Z", link[2], link[3], link[4]),
            path + " has an invalid composed cylinder contract"));
        failures.push(t_true(
            authored_rotation_ok(path, [link[1], 0.0, 0.0]),
            path + " has a detached link pose; expected rotateXYZ=" + link[1]));
    }
    let axles = [
        ["RockerL/Axle", 0.04, 0.25, [-0.125, -0.4, -1.2]],
        ["RockerR/Axle", 0.04, 0.25, [0.125, -0.4, -1.2]],
        ["BogieL/MiddleAxle", 0.035, 0.25, [-0.125, -0.2, -0.4]],
        ["BogieL/RearAxle", 0.035, 0.25, [-0.125, -0.2, 0.6]],
        ["BogieR/MiddleAxle", 0.035, 0.25, [0.125, -0.2, -0.4]],
        ["BogieR/RearAxle", 0.035, 0.25, [0.125, -0.2, 0.6]],
    ];
    for axle in axles {
        let path = ROOT + "/" + axle[0];
        failures.push(t_true(
            authored_cylinder_ok(path, "X", axle[1], axle[2], axle[3]),
            path + " has an invalid composed axle contract"));
    }
    let bearings = [
        ["RockerL/FrontAxleBearing", 0.085, [0.0, -0.4, -1.2]],
        ["RockerR/FrontAxleBearing", 0.085, [0.0, -0.4, -1.2]],
        ["BogieL/MiddleAxleBearing", 0.075, [0.0, -0.2, -0.4]],
        ["BogieL/RearAxleBearing", 0.075, [0.0, -0.2, 0.6]],
        ["BogieR/MiddleAxleBearing", 0.075, [0.0, -0.2, -0.4]],
        ["BogieR/RearAxleBearing", 0.075, [0.0, -0.2, 0.6]],
    ];
    for bearing in bearings {
        let path = ROOT + "/" + bearing[0];
        failures.push(t_true(
            authored_cylinder_ok(path, "X", bearing[1], 0.16, bearing[2]),
            path + " is missing its authored axle-bearing seat"));
    }
    let wheels = [
        "RockerL/Wheel_FL",
        "RockerR/Wheel_FR",
        "BogieL/Wheel_ML",
        "BogieL/Wheel_RL",
        "BogieR/Wheel_MR",
        "BogieR/Wheel_RR",
    ];
    for wheel in wheels {
        failures.push(t_true(
            wheel_shader_contract_ok(ROOT + "/" + wheel),
            ROOT + "/" + wheel + " still carries a retired wheel dust input"));
    }
    report_verdict(failures, "ROCKER BOGIE ANTENNA VISUAL", "ROCKER_BOGIE_ANTENNA_VISUAL");
}
