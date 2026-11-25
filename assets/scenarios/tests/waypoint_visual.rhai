// Graphics acceptance for the reusable waypoint marker presentation.
import "lunco://scripting/tools/waypoint_editor" as waypoint_editor;
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

fn simulation_dependencies(me, ctx) {
    #{ modelica_entities: [],
       entity_reads: [
           find_path_in_stage(me, ROUTE_POINT),
           find_path_in_stage(me, ROUTE_POINT + "/Dome"),
           find_path_in_stage(me, LANDING_LOCATION),
           find_path_in_stage(me, PREDICTED_LANDING),
       ],
       entity_writes: [find_path_in_stage(me, ROUTE_POINT + "/Dome")],
       query_reads: ["QueryUsdPrim"], required_inputs: [] }
}

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

fn annotation_shader_is_unlit(shader_path) {
    let shader = query("QueryUsdPrim", #{
        path: shader_path,
        attrs: ["inputs:diffuseColor", "inputs:specularColor", "inputs:emissiveColor"],
    });
    if shader == () || shader.ok == false || shader.attrs == () { return false; }
    let diffuse = shader.attrs["inputs:diffuseColor"];
    let specular = shader.attrs["inputs:specularColor"];
    let emissive = shader.attrs["inputs:emissiveColor"];
    diffuse == ()
        && specular != () && specular[0] == 0.0 && specular[1] == 0.0 && specular[2] == 0.0
        && emissive != () && emissive[0] == 0.0 && emissive[1] == 0.0 && emissive[2] == 0.0
}

fn annotation_shader_has_no_shader_opacity(shader_path) {
    let shader = query("QueryUsdPrim", #{
        path: shader_path,
        attrs: ["inputs:opacity", "inputs:opacityThreshold"],
    });
    shader != () && shader.ok != false && shader.attrs != ()
        && shader.attrs["inputs:opacity"] == ()
        && shader.attrs["inputs:opacityThreshold"] == ()
}

fn route_material_failure(entity) {
    let dome = query("QueryUsdPrim", #{
        path: ROUTE_POINT + "/Dome", relationships:true,
        attrs: ["primvars:doNotCastShadows", "primvars:displayOpacity", "lunco:surface:additive", "lunco:surface:unlit"],
    });
    if dome == () || dome.ok == false || dome.attrs == () || dome.relationships == () {
        return "Waypoint dome USD query failed: " + dome.to_string();
    }
    let attrs = dome.attrs;
    let mat = ROUTE_POINT + "/Looks/Dome_Mat";
    let opacity = attrs["primvars:displayOpacity"];
    if dome.relationships["material:binding"] != [mat] || attrs["primvars:doNotCastShadows"] != true ||
        attrs["lunco:surface:unlit"] != true || attrs["lunco:surface:additive"] != false ||
        opacity == () || opacity.len() != 1 || abs(opacity[0] - 0.42) > 0.000001 {
        return "Waypoint dome requires its bound, shadowless 42% opacity unlit material: " +
            attrs.to_string() + " binding=" + dome.relationships["material:binding"].to_string();
    }
    if !annotation_shader_is_unlit(mat + "/Surface") {
        return "The waypoint PreviewSurface supplies lighting or overrides the display colour: " + mat + "/Surface";
    }
    let look = get(entity, "PbrLook");
    if look == () || look.unlit != true || abs(look.base_color.alpha - opacity[0]) > 0.000001 {
        return "Projected waypoint material differs from the USD unlit/opacity inputs: " + look.to_string();
    }
    ""
}

fn route_point_uses_unvisited_color() {
    let shader = query("QueryUsdPrim", #{
        path: ROUTE_POINT + "/Dome",
        attrs: ["primvars:displayColor"],
    });
    if shader == () || shader.ok == false || shader.attrs == () { return false; }
    let color = shader.attrs["primvars:displayColor"];
    color != () && color.len() == 1 && color[0].len() == 3
        && (color[0][0] - 0.10).abs() < 0.000001
        && (color[0][1] - 0.95).abs() < 0.000001
        && (color[0][2] - 0.22).abs() < 0.000001
}

fn color_is(actual, expected) {
    actual != () && actual.len() == 3
        && (actual[0] - expected[0]).abs() < 0.000001
        && (actual[1] - expected[1]).abs() < 0.000001
        && (actual[2] - expected[2]).abs() < 0.000001
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
        && annotation_shader_has_no_shader_opacity(LANDING_LOCATION + "/Looks/Dome_Mat/Surface")
}

fn landing_location_has_no_trigger_collider() {
    let marker = query("QueryUsdPrim", #{ path: LANDING_LOCATION, topology: true });
    if marker == () || marker.ok == false || marker.topology == () ||
        marker.topology.parts == () || marker.topology.parts.len() == 0 { return false; }
    for part in marker.topology.parts {
        if part.collider != false { return false; }
    }
    true
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
        && annotation_shader_has_no_shader_opacity(PREDICTED_LANDING + "/Looks/Marker_Mat/Surface")
}

fn on_start(me, ctx) {
    this.rover = find_path_in_stage(me, "/WaypointVisual/Rover");
    this.marker = find_path_in_stage(me, ROUTE_POINT);
    this.dome = find_path_in_stage(me, ROUTE_POINT + "/Dome");
    this.landing = find_path_in_stage(me, LANDING_LOCATION);
    this.predicted = find_path_in_stage(me, PREDICTED_LANDING);
    this.done = false;
    this.phase = 0;
    this.t = 0.0;
    this.doc = -1;
    this.checks = [];
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    let checks = [];
    if this.phase == 0 {
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
        checks.push(route_material_failure(this.dome));
        checks.push(t_true(route_point_uses_unvisited_color(),
            "route point does not expose its green unvisited-state colour"));
        checks.push(t_true(landing_location_uses_annotation_material(),
            "landing location does not expose the emissive shadowless annotation material"));
        checks.push(t_true(landing_location_has_no_trigger_collider(),
            "landing location is not a visual-only ground mark"));
        checks.push(t_true(predicted_landing_uses_annotation_material(),
            "predicted landing does not expose the emissive shadowless annotation material"));

        let route = query("QueryUsdPrim", #{ path: "/WaypointVisual", children: true });
        if route == () || route.ok == false || route.doc_id == () || route.document_generation == () {
            checks.push(t_true(false, "waypoint review document is unavailable"));
            this.done = true;
            report_verdict(checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
            return;
        }
        this.doc = route.doc_id;
        let starts_green = color_is(
            t_pbr_base_color(this.dome), [0.10, 0.95, 0.22]);
        checks.push(t_true(starts_green,
            "the unvisited waypoint sphere renders green"));
        if !starts_green {
            this.done = true;
            report_verdict(checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
            return;
        }

        this.checks = checks;
        this.generation = route.document_generation;
        let visited = #{};
        visited[ROUTE_POINT] = true;
        // This scene contains a standalone marker. Supply that marker and its
        // actual point palette to the same single-point update used on a route visit.
        let point = query("QueryUsdPrim", #{doc_id:this.doc, path:ROUTE_POINT, attrs:["primvars:displayColor"]});
        let style = waypoint_editor::record_color(point);
        if !style.ok {
            this.checks.push("Cannot read standalone waypoint palette: " + style.to_string());
            this.done = true;
            report_verdict(this.checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
            return;
        }
        let updated = waypoint_editor::sync_marker_snapshot(this.doc,
            #{ok:true, paths:[ROUTE_POINT], marker_colors:[style.color]}, visited);
        if updated == () || updated.ok != true {
            this.checks.push(t_true(false,
                "Visited waypoint display update failed: " + updated.to_string()));
            this.done = true;
            report_verdict(this.checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
            return;
        }
        this.phase = 1;
        return;
    }

    let rendered = t_pbr_base_color(this.dome);
    let turns_gray = color_is(rendered, [0.55, 0.55, 0.55]);
    if turns_gray {
        this.checks.push(t_true(route_point_uses_unvisited_color()
            && usd_document_generation(this.doc) == this.generation,
            "the visited waypoint sphere renders gray while its USD colour and generation stay unchanged"));
        this.done = true;
        report_verdict(this.checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
    } else if this.t > 10.0 {
        this.checks.push(t_true(false,
            "Visited dome material expected [0.55, 0.55, 0.55], received " + rendered.to_string()));
        this.done = true;
        report_verdict(this.checks, "WAYPOINT VISUAL", "WAYPOINT_VISUAL");
    }
}
