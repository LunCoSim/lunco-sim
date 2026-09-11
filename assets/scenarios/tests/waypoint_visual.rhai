// Graphics acceptance for the reusable waypoint marker presentation.
const TEST_KIND = "graphics";
const ROUTE_POINT = "/WaypointVisual/Waypoint1";
const LANDING_LOCATION = "/WaypointVisual/LandingLocation";
const PREDICTED_LANDING = "/WaypointVisual/PredictedLanding";
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

fn route_point_uses_shared_billboard_contract() {
    let record = query("QueryUsdPrim", #{
        path: ROUTE_POINT,
        attrs: ["lunco:billboard", "lunco:billboard:text", "lunco:billboard:offsetY", "lunco:billboard:fadeEnd"],
    });
    record != () && record.ok != false && record.attrs != ()
        && record.attrs["lunco:billboard"] == true
        && record.attrs["lunco:billboard:text"] != ()
        && record.attrs["lunco:billboard:offsetY"] == 4.0
        && record.attrs["lunco:billboard:fadeEnd"] == 1200.0
}

fn annotation_shader_is_emissive(shader_path) {
    let shader = query("QueryUsdPrim", #{
        path: shader_path,
        attrs: ["inputs:diffuseColor", "inputs:specularColor", "inputs:emissiveColor"],
    });
    if shader == () || shader.ok == false || shader.attrs == () { return false; }
    let diffuse = shader.attrs["inputs:diffuseColor"];
    let specular = shader.attrs["inputs:specularColor"];
    let emissive = shader.attrs["inputs:emissiveColor"];
    diffuse != () && diffuse[0] == 0.0 && diffuse[1] == 0.0 && diffuse[2] == 0.0
        && specular != () && specular[0] == 0.0 && specular[1] == 0.0 && specular[2] == 0.0
        && emissive != () && (emissive[0] > 0.0 || emissive[1] > 0.0 || emissive[2] > 0.0)
}

fn annotation_shader_is_opaque(shader_path) {
    let shader = query("QueryUsdPrim", #{
        path: shader_path,
        attrs: ["inputs:opacity", "inputs:opacityThreshold"],
    });
    shader != () && shader.ok != false && shader.attrs != ()
        && shader.attrs["inputs:opacity"] == ()
        && shader.attrs["inputs:opacityThreshold"] == ()
}

fn route_point_uses_annotation_material() {
    let dome = query("QueryUsdPrim", #{
        path: ROUTE_POINT + "/Dome",
        attrs: ["primvars:doNotCastShadows", "primvars:displayOpacity", "lunco:surface:additive"],
    });
    if dome == () || dome.ok == false || dome.attrs == () { return false; }
    let opacity = dome.attrs["primvars:displayOpacity"];
    dome.attrs["primvars:doNotCastShadows"] == true
        && opacity != () && opacity.len() == 1 && opacity[0] == 0.08
        && dome.attrs["lunco:surface:additive"] == true
        && annotation_shader_is_emissive(ROUTE_POINT + "/Looks/Dome_Mat/Surface")
}

fn landing_location_uses_annotation_material() {
    let dome = query("QueryUsdPrim", #{
        path: LANDING_LOCATION + "/Dome",
        attrs: ["primvars:doNotCastShadows", "physics:collisionEnabled"],
    });
    dome != () && dome.ok != false && dome.attrs != ()
        && dome.attrs["primvars:doNotCastShadows"] == true
        && dome.attrs["physics:collisionEnabled"] == false
        && annotation_shader_is_emissive(LANDING_LOCATION + "/Looks/Dome_Mat/Surface")
        && annotation_shader_is_opaque(LANDING_LOCATION + "/Looks/Dome_Mat/Surface")
}

fn predicted_landing_uses_annotation_material() {
    let parts = ["PX", "NX", "PZ", "NZ", "Center"];
    for part in parts {
        let record = query("QueryUsdPrim", #{
            path: PREDICTED_LANDING + "/Brackets/" + part,
            attrs: ["primvars:doNotCastShadows", "physics:collisionEnabled"],
        });
        if record == () || record.ok == false || record.attrs == ()
            || record.attrs["primvars:doNotCastShadows"] != true
            || record.attrs["physics:collisionEnabled"] != false { return false; }
    }
    annotation_shader_is_emissive(PREDICTED_LANDING + "/Looks/Marker_Mat/Surface")
        && annotation_shader_is_opaque(PREDICTED_LANDING + "/Looks/Marker_Mat/Surface")
}

fn on_start(me) {
    this.rover = find("/WaypointVisual/Rover");
    this.marker = find(ROUTE_POINT);
    this.landing = find(LANDING_LOCATION);
    this.predicted = find(PREDICTED_LANDING);
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
        "reusable route-point marker is missing from the review scene"));
    checks.push(t_true(this.landing >= 0 && this.predicted >= 0,
        "reusable landing annotations are missing from the review scene"));
    checks.push(t_true(sky_uses_shader_defaults(),
        "waypoint review authors a redundant procedural-sky input override"));
    checks.push(t_true(route_point_uses_shared_billboard_contract(),
        "route-point marker does not expose the shared bounded-label contract"));
    checks.push(t_true(route_point_uses_annotation_material(),
        "route point does not expose the emissive shadowless annotation material"));
    checks.push(t_true(landing_location_uses_annotation_material(),
        "landing location does not expose the emissive shadowless annotation material"));
    checks.push(t_true(predicted_landing_uses_annotation_material(),
        "predicted landing does not expose the emissive shadowless annotation material"));
    report_verdict(checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
}
