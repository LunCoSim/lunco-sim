// Graphics acceptance for the reusable waypoint marker presentation.
const TEST_KIND = "graphics";
const WAYPOINT = "/WaypointVisual/Waypoint1";
const SKY_SHADER = "/WaypointVisual/Sky/Looks/Starfield/Shader";
const SKY_INPUTS = [
    "inputs:core_color",
    "inputs:arm_color",
    "inputs:star_density",
    "inputs:limit_magnitude",
    "inputs:magnitude_slope",
    "inputs:color_spread",
    "inputs:point_size",
    "inputs:glow",
    "inputs:brightness",
    "inputs:seed",
    "inputs:band_intensity",
    "inputs:core_boost",
    "inputs:band_width",
    "inputs:dust_strength",
    "inputs:dust_scale",
    "inputs:pole_tilt",
    "inputs:pole_azimuth",
    "inputs:center_roll",
];

fn sky_uses_shader_defaults() {
    let record = query("QueryUsdPrim", #{
        path: SKY_SHADER,
        attrs: SKY_INPUTS,
    });
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    for attr in SKY_INPUTS {
        if record.attrs[attr] != () {
            return false;
        }
    }
    true
}

fn waypoint_uses_shared_billboard_contract() {
    let record = query("QueryUsdPrim", #{
        path: WAYPOINT,
        attrs: ["lunco:billboard", "lunco:billboard:offsetY", "lunco:billboard:fadeEnd"],
    });
    record != () && record.ok != false && record.attrs != ()
        && record.attrs["lunco:billboard"] == true
        && record.attrs["lunco:billboard:text"] != ()
        && record.attrs["lunco:billboard:offsetY"] == 4.0
        && record.attrs["lunco:billboard:fadeEnd"] == 1200.0
}

fn on_start(me) {
    this.rover = find("/WaypointVisual/Rover");
    this.marker = find("/WaypointVisual/Waypoint1");
    this.done = false;
    this.t = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    let checks = [];
    checks.push(t_true(this.rover >= 0,
        "six-wheel rover is missing from the waypoint review scene"));
    checks.push(t_true(this.marker >= 0,
        "reusable waypoint marker is missing from the review scene"));
    checks.push(t_true(sky_uses_shader_defaults(),
        "waypoint review authors a redundant procedural-sky input override"));
    checks.push(t_true(waypoint_uses_shared_billboard_contract(),
        "waypoint marker does not expose the shared bounded-label contract"));
    report_verdict(checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
}
