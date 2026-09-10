// Griffin-1 Twin requirements, lints, and live-edit gates.
//
// This is Twin policy, not a second USD reader. Every model fact comes from
// QueryUsdPrim, and every live edit goes through the typed USD command bus.
// Pass a numeric `doc_id` for an open Editor document; pass `()` for the one
// mounted runtime stage. The same functions therefore exercise the model in
// the headful Editor and in a Twin-owned scene test.

// Tool-library functions are used instead of top-level constants because the
// production static-module binding resolves names from function scope.
fn max_payload_kg() { griffin_spec::max_payload_kg() }
fn max_ramp_command_rad() { griffin_spec::max_ramp_command_rad() }
fn pi_value() { griffin_spec::pi_value() }

fn _query(doc_id, path, attrs, rels, children) {
    let request = #{ path: path };
    if doc_id != () { request.doc_id = doc_id; }
    if attrs != () { request.attrs = attrs; }
    if rels != () { request.rels = rels; }
    if children != () { request.children = children; }
    query("QueryUsdPrim", request)
}

fn _ok(record) {
    record != () && record.ok != false && record.path != ()
}

fn _has(values, expected) {
    if values == () { return false; }
    for value in values {
        if value == expected { return true; }
    }
    false
}

fn _relationship(record, name) {
    if !_ok(record) || record.relationships == ()
        || record.relationships[name] == ()
        || record.relationships[name].len() != 1 {
        return ();
    }
    record.relationships[name][0]
}

fn _finding(id, path, message) {
    #{ id: id, severity: "error", path: path, message: message }
}

// Generic assembly-audit primitives. The Griffin manifest below supplies the
// Twin-specific part names and known values; these checks deliberately know
// nothing about Griffin so the same helpers can be reused by another Twin.
fn _visible(record) {
    _ok(record) && record.attrs["visibility"] != "invisible"
}

fn _geometry_type(type_name) {
    _has(["Mesh", "Cube", "Cylinder", "Cone", "Sphere", "Capsule",
          "NurbsPatch", "Points", "BasisCurves"], type_name)
}

fn _near(a, b, tolerance) {
    (a - b).abs() <= tolerance
}

fn _near_vec(a, b, tolerance) {
    if a == () || b == () || a.len() != b.len() { return false; }
    for i in 0..a.len() {
        if !_near(a[i], b[i], tolerance) { return false; }
    }
    true
}

fn _horizontal_radius_squared(position) {
    if position == () || position.len() < 3 { return -1.0; }
    position[0] * position[0] + position[2] * position[2]
}

fn _layout_query(doc_id, path, attrs, children) {
    _query(doc_id, path, attrs, (), children)
}

fn _layout_finding(id, path, message) {
    _finding(id, path, message)
}

// Visual-only wrappers are not always spawned by the isolated headless
// requirements fixture. Prefer the resolved world pose for a live Editor
// document, but retain an authored local datum for a composed contract test.
// This is still a typed USD read; it never invents a placement.
fn _position(record) {
    if record == () { return (); }
    let world = record.world_position;
    if world != () && world.len() == 3 { return world; }
    if record.attrs != () {
        let local = record.attrs["xformOp:translate"];
        if local != () && local.len() == 3 { return local; }
    }
    ()
}

// Layout contracts describe the authored assembly, not a body after gravity
// or contact dynamics have moved it. Prefer the explicit local datum when it
// exists; fall back to the resolved pose for references that do not publish a
// local translate opinion.
fn _authored_position(record) {
    if record != () && record.attrs != () {
        let local = record.attrs["xformOp:translate"];
        if local != () && local.len() == 3 { return local; }
    }
    _position(record)
}

fn _layout_prim_exists(doc_id, root, name, rule, findings) {
    let path = root + "/" + name;
    if !_ok(_layout_query(doc_id, path, [], false)) {
        findings.push(_layout_finding(rule, path, "required named part is missing"));
    }
}

fn _pair_mirror_check(findings, rule, left_path, left, right_path, right,
    axis, label) {
    if !_ok(left) || !_ok(right) { return; }
    let a = _authored_position(left);
    let b = _authored_position(right);
    if a == () || b == () || a.len() != 3 || b.len() != 3 { return; }
    if axis == "X" {
        if !_near(a[0], -b[0], 0.02) || !_near(a[1], b[1], 0.02)
            || !_near(a[2], b[2], 0.02) {
            findings.push(_layout_finding(rule, left_path,
                label + " must mirror " + right_path + " across the X=0 plane"));
        }
    }
    if axis == "Z" {
        if !_near(a[2], -b[2], 0.02) || !_near(a[1], b[1], 0.02)
            || !_near(a[0], b[0], 0.02) {
            findings.push(_layout_finding(rule, left_path,
                label + " must mirror " + right_path + " across the Z=0 plane"));
        }
    }
}

fn _solar_layout_report(doc_id, root) {
    let findings = [];
    let port_path = root + "/SolarPanelPort";
    let starboard_path = root + "/SolarPanelStarboard";
    let port = _layout_query(doc_id, port_path,
        ["visibility", "xformOp:translate", "xformOp:rotateXYZ"], true);
    let starboard = _layout_query(doc_id, starboard_path,
        ["visibility", "xformOp:translate", "xformOp:rotateXYZ"], true);
    if !_ok(port) || !_ok(starboard) {
        return findings;
    }
    _pair_mirror_check(findings, "GR-016", starboard_path, starboard,
        port_path, port, "Z", "solar arrays");
    let port_pos = _authored_position(port);
    let starboard_pos = _authored_position(starboard);
    if port_pos == () || starboard_pos == ()
        || port_pos.len() != 3 || starboard_pos.len() != 3 {
        findings.push(_layout_finding("GR-016", root,
            "solar-array world placement is unavailable in the composed stage"));
        return findings;
    }
    if port_pos[2] >= 0.0 || starboard_pos[2] <= 0.0 {
        findings.push(_layout_finding("GR-016", root,
            "port/starboard solar arrays must occupy opposite signed Z sides"));
    }
    if !_near(port_pos[0], 0.0, 0.02) || !_near(starboard_pos[0], 0.0, 0.02) {
        findings.push(_layout_finding("GR-016", root,
            "solar arrays must attach to the side-wall datum, not the fore/aft axis"));
    }
    if !_has(port.children, port_path + "/Cells")
        || !_has(starboard.children, starboard_path + "/Cells") {
        findings.push(_layout_finding("GR-016", root,
            "each solar panel must have exactly its named Cells geometry"));
    }
    findings
}

fn _ramp_layout_report(doc_id, root) {
    let findings = [];
    let port_path = root + "/EgressRampPort";
    let starboard_path = root + "/EgressRampStarboard";
    let port = _layout_query(doc_id, port_path,
        ["xformOp:translate", "xformOp:rotateXYZ", "lunco:deployment_angle_rad"], true);
    let starboard = _layout_query(doc_id, starboard_path,
        ["xformOp:translate", "xformOp:rotateXYZ", "lunco:deployment_angle_rad"], true);
    if !_ok(port) || !_ok(starboard) {
        return findings;
    }
    _pair_mirror_check(findings, "GR-017", port_path, port,
        starboard_path, starboard, "X", "egress ramps");
    let port_pos = _authored_position(port);
    let starboard_pos = _authored_position(starboard);
    if port_pos == () || starboard_pos == ()
        || port_pos.len() != 3 || starboard_pos.len() != 3 {
        findings.push(_layout_finding("GR-017", root,
            "ramp world placement is unavailable in the composed stage"));
        return findings;
    }
    if port_pos[0] <= 0.0 || starboard_pos[0] >= 0.0
        || !_near(port_pos[2], 0.0, 0.02)
        || !_near(starboard_pos[2], 0.0, 0.02) {
        findings.push(_layout_finding("GR-017", root,
            "port/starboard ramps must be opposite side ramps on the X axis"));
    }
    let port_rot = port.attrs["xformOp:rotateXYZ"];
    let starboard_rot = starboard.attrs["xformOp:rotateXYZ"];
    if port_rot != () && starboard_rot != () {
        if !_near(port_rot[0], 0.0, 0.02) || !_near(port_rot[1], 0.0, 0.02)
            || !_near(starboard_rot[0], 0.0, 0.02)
            || (!_near(starboard_rot[1], 0.0, 0.02)
                && !_near(starboard_rot[1], 180.0, 0.02))
            || !_near(port_rot[2], -starboard_rot[2], 0.02) {
            findings.push(_layout_finding("GR-017", root,
                "ramp rotations must mirror about the Z hinge axis"));
        }
    }
    let port_angle = port.attrs["lunco:deployment_angle_rad"];
    let starboard_angle = starboard.attrs["lunco:deployment_angle_rad"];
    if port_angle != () && starboard_angle != () {
        if !_near(port_angle, -starboard_angle, 0.002) {
            findings.push(_layout_finding("GR-017", root,
                "ramp deployment metadata must be equal and opposite"));
        }
        if port_rot != () && !_near(port_rot[2] * pi_value() / 180.0,
            port_angle, 0.02) {
            findings.push(_layout_finding("GR-017", port_path,
                "ramp transform rotation and deployment metadata disagree"));
        }
        if starboard_rot != () && !_near(starboard_rot[2] * pi_value() / 180.0,
            starboard_angle, 0.02) {
            findings.push(_layout_finding("GR-017", starboard_path,
                "ramp transform rotation and deployment metadata disagree"));
        }
    }
    if !_has(port.children, port_path + "/Surface")
        || !_has(starboard.children, starboard_path + "/Surface") {
        findings.push(_layout_finding("GR-017", root,
            "each ramp must have its own named contact surface"));
    }
    findings
}

fn _body_shape_report(doc_id, root) {
    let findings = [];
    let body_path = root + "/BoxyAirframe";
    let deck_path = root + "/IsogridDeck/DeckPlate";
    let body = _layout_query(doc_id, body_path,
        ["size", "xformOp:scale", "physics:collisionEnabled", "visibility"], false);
    let deck = _layout_query(doc_id, deck_path,
        ["size", "xformOp:scale", "xformOp:translate", "physics:collisionEnabled"], false);
    if !_ok(body) {
        findings.push(_layout_finding("GR-018", body_path,
            "visible boxy Griffin airframe is missing"));
    } else {
        if body.type_name != "Cube" {
            findings.push(_layout_finding("GR-018", body_path,
                "primary Griffin airframe must be a boxy Cube study shape"));
        }
        let scale = body.attrs["xformOp:scale"];
        let size = body.attrs["size"];
        if scale == () || scale.len() != 3 || size == () || size <= 0.0 {
            findings.push(_layout_finding("GR-018", body_path,
                "boxy airframe needs positive three-axis dimensions"));
        } else {
            let width = scale[0].abs() * size;
            let height = scale[1].abs() * size;
            let depth = scale[2].abs() * size;
            if width <= height || depth <= height {
                findings.push(_layout_finding("GR-018", body_path,
                    "Griffin body must be wider and deeper than it is tall"));
            }
        }
        if body.attrs["physics:collisionEnabled"] != true {
            findings.push(_layout_finding("GR-018", body_path,
                "boxy airframe must own an enabled collider"));
        }
        if body.attrs["visibility"] == "invisible" {
            findings.push(_layout_finding("GR-018", body_path,
                "boxy airframe must be visible in the authored preview"));
        }
    }
    if !_ok(deck) {
        findings.push(_layout_finding("GR-018", deck_path,
            "rectangular isogrid deck plate is missing"));
    } else {
        if deck.type_name != "Cube" {
            findings.push(_layout_finding("GR-018", deck_path,
                "payload deck plate must be a rectangular Cube"));
        }
        let deck_scale = deck.attrs["xformOp:scale"];
        if deck_scale == () || deck_scale.len() != 3
            || deck_scale[0] <= 0.0 || deck_scale[1] <= 0.0
            || deck_scale[2] <= 0.0 {
            findings.push(_layout_finding("GR-018", deck_path,
                "payload deck plate needs positive dimensions"));
        }
        if deck.attrs["physics:collisionEnabled"] != true {
            findings.push(_layout_finding("GR-018", deck_path,
                "payload deck plate must own an enabled collider"));
        }
    }
    findings
}

fn _tank_layout_report(doc_id, root) {
    let findings = [];
    let names = ["TankPX", "TankNX", "TankPZ", "TankNZ"];
    let records = [];
    for name in names {
        let path = root + "/" + name;
        let record = _layout_query(doc_id, path,
            ["visibility", "radius", "height", "axis", "xformOp:translate"], false);
        records.push(record);
        if !_ok(record) {
            findings.push(_layout_finding("GR-033", path,
                "required propellant tank is missing"));
            continue;
        }
        if !_visible(record) {
            findings.push(_layout_finding("GR-033", path,
                "all four Griffin propellant tanks must be visible"));
        }
        if record.type_name != "Capsule" || record.attrs["axis"] != "Y"
            || record.attrs["radius"] == () || record.attrs["radius"] <= 0.0
            || record.attrs["height"] == () || record.attrs["height"] <= 0.0 {
            findings.push(_layout_finding("GR-033", path,
                "propellant tank must be a positive Y-axis Capsule"));
        }
    }
    if _ok(records[0]) && _ok(records[1]) {
        _pair_mirror_check(findings, "GR-033", root + "/TankPX", records[0],
            root + "/TankNX", records[1], "X", "X-axis propellant tanks");
    }
    if _ok(records[2]) && _ok(records[3]) {
        _pair_mirror_check(findings, "GR-033", root + "/TankPZ", records[2],
            root + "/TankNZ", records[3], "Z-axis propellant tanks");
    }
    findings
}

fn _engine_layout_report(doc_id, root) {
    let findings = [];
    let cluster_path = root + "/Nozzle/MainEngineCluster";
    let cluster = _layout_query(doc_id, cluster_path, [], true);
    if !_ok(cluster) {
        findings.push(_layout_finding("GR-032", cluster_path,
            "seven-engine visual cluster is missing"));
        return findings;
    }
    if cluster.type_name != "Xform" {
        findings.push(_layout_finding("GR-032", cluster_path,
            "main engine cluster must be an Xform"));
    }
    let engine_count = 0;
    for i in 1..8 {
        let path = cluster_path + "/Engine" + (if i < 10 { "0" + i } else { i.to_string() });
        let engine = _layout_query(doc_id, path,
            ["visibility", "radius1", "radius2", "height", "physics:collisionEnabled"], false);
        if !_ok(engine) {
            findings.push(_layout_finding("GR-032", path,
                "required main engine bell is missing"));
            continue;
        }
        engine_count += 1;
        if engine.type_name != "Cone" || !_visible(engine)
            || engine.attrs["radius1"] == () || engine.attrs["radius1"] <= 0.0
            || engine.attrs["radius2"] == () || engine.attrs["radius2"] <= 0.0
            || engine.attrs["height"] == () || engine.attrs["height"] <= 0.0 {
            findings.push(_layout_finding("GR-032", path,
                "each main engine must be a visible positive Cone bell"));
        }
        if engine.attrs["physics:collisionEnabled"] == true {
            findings.push(_layout_finding("GR-032", path,
                "engine visual bell must not own a collider"));
        }
    }
    if engine_count != griffin_spec::main_engine_count() {
        findings.push(_layout_finding("GR-032", cluster_path,
            "expected exactly seven main engine bells"));
    }
    findings
}

fn _leg_layout_report(doc_id, root) {
    let findings = [];
    let names = ["LegPX", "LegNX", "LegPZ", "LegNZ"];
    let pad_names = ["PadPX", "PadNX", "PadPZ", "PadNZ"];
    let legs = [];
    for name in names {
        legs.push(_layout_query(doc_id, root + "/" + name,
            ["xformOp:translate", "xformOp:rotateXYZ", "physics:mass"], true));
    }
    for i in 0..legs.len() {
        let leg = legs[i];
        let path = root + "/" + names[i];
        if !_ok(leg) { continue; }
        let pos = _authored_position(leg);
        if pos == () || pos.len() != 3 { continue; }
        let dynamic_pos = _position(leg);
        if dynamic_pos == () || dynamic_pos.len() != 3 { dynamic_pos = pos; }
        if i == 0 && (pos[0] <= 0.0 || !_near(pos[2], 0.0, 0.02)) {
            findings.push(_layout_finding("GR-019", path, "LegPX must attach on +X"));
        }
        if i == 1 && (pos[0] >= 0.0 || !_near(pos[2], 0.0, 0.02)) {
            findings.push(_layout_finding("GR-019", path, "LegNX must attach on -X"));
        }
        if i == 2 && (pos[2] <= 0.0 || !_near(pos[0], 0.0, 0.02)) {
            findings.push(_layout_finding("GR-019", path, "LegPZ must attach on +Z"));
        }
        if i == 3 && (pos[2] >= 0.0 || !_near(pos[0], 0.0, 0.02)) {
            findings.push(_layout_finding("GR-019", path, "LegNZ must attach on -Z"));
        }
        let pad_path = path + "/" + pad_names[i];
        let pad = _layout_query(doc_id, pad_path,
            ["axis", "radius", "height", "physics:collisionEnabled",
             "xformOp:translate"], false);
        if !_ok(pad) { continue; }
        let pad_pos = _position(pad);
        if pad_pos == () || pad_pos.len() != 3 { continue; }
        if pad_pos[1] >= dynamic_pos[1] {
            findings.push(_layout_finding("GR-019", pad_path,
                "foot pad must be below its leg attachment"));
        }
        if _horizontal_radius_squared(pad_pos)
            <= _horizontal_radius_squared(dynamic_pos) {
            findings.push(_layout_finding("GR-019", pad_path,
                "foot pad must be farther outboard than its leg attachment"));
        }
        if pad.attrs["axis"] != "Y" || pad.attrs["radius"] == ()
            || pad.attrs["radius"] <= 0.0 || pad.attrs["height"] == ()
            || pad.attrs["height"] <= 0.0 {
            findings.push(_layout_finding("GR-019", pad_path,
                "foot pad needs a positive Y-axis cylindrical contact shape"));
        }
        if pad.attrs["physics:collisionEnabled"] != true {
            findings.push(_layout_finding("GR-019", pad_path,
                "foot pad must keep collision enabled"));
        }
    }
    if _ok(legs[0]) && _ok(legs[1]) {
        _pair_mirror_check(findings, "GR-019", root + "/LegPX", legs[0],
            root + "/LegNX", legs[1], "X", "X-axis landing legs");
    }
    if _ok(legs[2]) && _ok(legs[3]) {
        _pair_mirror_check(findings, "GR-019", root + "/LegPZ", legs[2],
            root + "/LegNZ", legs[3], "Z", "Z-axis landing legs");
    }
    if _ok(legs[0]) && _ok(legs[2]) {
        let px = _horizontal_radius_squared(_authored_position(legs[0]));
        let pz = _horizontal_radius_squared(_authored_position(legs[2]));
        if !_near(px, pz, 0.05) {
            findings.push(_layout_finding("GR-019", root,
                "all four leg attachment points must share one radial body datum"));
        }
    }
    findings
}

fn _legacy_visibility_report(doc_id, root) {
    let findings = [];
    // These are obsolete source/proxy prims. They must be removed from the
    // Griffin composition, not merely hidden, so a later consumer cannot
    // accidentally reactivate a second body.
    for name in ["SolarWingPX", "SolarWingNX",
                 "Hull", "UpperDeck",
                 "LandingLegFrontPort", "LandingLegRearPort",
                 "LandingLegFrontStarboard", "LandingLegRearStarboard",
                 "SolidLegFrontPort", "SolidLegRearPort",
                 "SolidLegFrontStarboard", "SolidLegRearStarboard",
                 "SolidFootFrontPort", "SolidFootRearPort",
                 "SolidFootFrontStarboard", "SolidFootRearStarboard"] {
        let path = root + "/" + name;
        let record = _layout_query(doc_id, path, ["visibility"], false);
        if _ok(record) {
            findings.push(_layout_finding("GR-020", path,
                "obsolete comparison/proxy geometry must be deleted, not hidden"));
        }
    }
    let bell = _layout_query(doc_id, root + "/Nozzle/Bell", ["visibility"], false);
    if _ok(bell) {
        findings.push(_layout_finding("GR-020", root + "/Nozzle/Bell",
            "single-bell placeholder must be deleted after the seven-engine cluster is authored"));
    }
    findings
}

/// Check counts, side placement, symmetry, body shape, leg attachment, and
/// suppression of inherited comparison geometry. These checks are relational,
/// so they catch a correctly named part mounted on the wrong side as well as a
/// missing part.
fn layout_report(doc_id, root) {
    let findings = [];
    for result in [_solar_layout_report(doc_id, root),
                   _ramp_layout_report(doc_id, root),
                   _body_shape_report(doc_id, root),
                   _leg_layout_report(doc_id, root),
                   _tank_layout_report(doc_id, root),
                   _engine_layout_report(doc_id, root),
                   _legacy_visibility_report(doc_id, root)] {
        for finding in result { findings.push(finding); }
    }
    #{ok: findings.len() == 0, root: root, doc_id: doc_id, findings: findings}
}

fn _audit_parameter(doc_id, rule, part_path, parameter) {
    let findings = [];
    let parameter_path = parameter.path;
    if parameter_path == () { parameter_path = part_path; }
    let record = _query(doc_id, parameter_path, [parameter.attr], (), false);
    let value = ();
    if !_ok(record) {
        findings.push(_finding(rule, parameter_path,
            "parameter owner is missing for " + parameter.name));
        return findings;
    }
    value = record.attrs[parameter.attr];
    if value == () {
        findings.push(_finding(rule, parameter_path,
            "missing required parameter " + parameter.attr + " (" + parameter.name + ")"));
        return findings;
    }
    if parameter.nonempty == true && value == "" {
        findings.push(_finding(rule, parameter_path,
            "parameter " + parameter.attr + " must not be empty"));
    }
    if parameter.contains != () && !value.contains(parameter.contains) {
        findings.push(_finding(rule, parameter_path,
            "parameter " + parameter.attr + " must contain '" + parameter.contains + "'"));
    }
    if parameter.exact != () && value != parameter.exact {
        findings.push(_finding(rule, parameter_path,
            "parameter " + parameter.attr + " has an unexpected value"));
    }
    if parameter.positive == true && value <= 0.0 {
        findings.push(_finding(rule, parameter_path,
            "parameter " + parameter.attr + " must be positive"));
    }
    if parameter.max_abs != () && value.abs() > parameter.max_abs {
        findings.push(_finding(rule, parameter_path,
            "parameter " + parameter.attr + " exceeds its authored absolute limit"));
    }
    findings
}

fn _audit_part(doc_id, spec) {
    let findings = [];
    let record = _query(doc_id, spec.path, spec.attrs, (), true);
    if !_ok(record) {
        findings.push(_finding(spec.rule, spec.path,
            spec.label + " is missing from the composed stage"));
        return findings;
    }
    if spec.expected_type != () && record.type_name != spec.expected_type {
        findings.push(_finding(spec.rule, spec.path,
            spec.label + " must be " + spec.expected_type
                + ", got " + record.type_name));
    }
    if spec.geometry == true && !_geometry_type(record.type_name) {
        findings.push(_finding(spec.rule, spec.path,
            spec.label + " must resolve to a geometric prim, got " + record.type_name));
    }
    if spec.visible == true && !_visible(record) {
        findings.push(_finding(spec.rule, spec.path,
            spec.label + " must be visible in the authored preview"));
    }
    if spec.collider == true && record.attrs["physics:collisionEnabled"] != true {
        findings.push(_finding(spec.rule, spec.path,
            spec.label + " must own an enabled collider"));
    }
    if spec.required_children != () {
        for child_path in spec.required_children {
            if !_ok(_query(doc_id, child_path, [], (), false)) {
                findings.push(_finding(spec.rule, child_path,
                    spec.label + " is missing required child geometry/interface"));
            }
        }
    }
    if spec.geometry_children != () {
        for geometry in spec.geometry_children {
            let geometry_record = _query(doc_id, geometry.path,
                ["visibility", "physics:collisionEnabled"], (), false);
            if !_ok(geometry_record) {
                findings.push(_finding(spec.rule, geometry.path,
                    spec.label + " is missing required geometry " + geometry.label));
                continue;
            }
            if geometry.geometry == true && !_geometry_type(geometry_record.type_name) {
                findings.push(_finding(spec.rule, geometry.path,
                    "required geometry " + geometry.label + " has non-geometric type "
                        + geometry_record.type_name));
            }
            if geometry.visible == true && !_visible(geometry_record) {
                findings.push(_finding(spec.rule, geometry.path,
                    "required geometry " + geometry.label + " is hidden"));
            }
            if geometry.collider == true
                && geometry_record.attrs["physics:collisionEnabled"] != true {
                findings.push(_finding(spec.rule, geometry.path,
                    "required geometry " + geometry.label + " must own an enabled collider"));
            }
        }
    }
    if spec.parameters != () {
        for parameter in spec.parameters {
            for finding in _audit_parameter(doc_id, spec.rule, spec.path, parameter) {
                findings.push(finding);
            }
        }
    }
    findings
}

// This is the single source of known Griffin part contracts. Numeric values
// that are not source-backed are marked as study/TBD in the returned metadata;
// the linter checks only explicit values and geometry obligations.
fn part_manifest(root) {
    let legs = [];
    for name in ["LegPX", "LegNX", "LegPZ", "LegNZ"] {
        let pad_name = "Pad" + name.sub_string(3, name.len() - 3);
        let leg_path = root + "/" + name;
        legs.push(#{
            id: "landing_leg_" + name,
            label: "landing leg " + name,
            rule: "GR-010",
            path: leg_path,
            attrs: ["visibility", "physics:mass"],
            expected_type: "Xform",
            visible: true,
            required_children: [leg_path + "/Strut", leg_path + "/" + pad_name],
            geometry_children: [#{
                label: "foot pad",
                path: leg_path + "/" + pad_name,
                geometry: true,
                visible: true,
                collider: true,
            }],
            parameters: [#{
                name: "leg mass",
                attr: "physics:mass",
                positive: true,
            }],
            parameter_status: "inherited study mass; as-built value TBD",
        });
    }

    let ramps = [];
    for name in ["EgressRampPort", "EgressRampStarboard"] {
        let ramp_path = root + "/" + name;
        ramps.push(#{
            id: "egress_ramp_" + name,
            label: "egress ramp " + name,
            rule: "GR-003",
            path: ramp_path,
            attrs: ["visibility", "physics:mass", "lunco:mechanism",
                    "lunco:deployment_angle_rad"],
            expected_type: "Xform",
            visible: true,
            required_children: [
                ramp_path + "/Surface",
                ramp_path + "/EdgeRail",
                ramp_path + "/EdgeRailInner",
            ],
            geometry_children: [
                #{label: "surface", path: ramp_path + "/Surface", geometry: true, visible: true, collider: true},
                #{label: "outer edge rail", path: ramp_path + "/EdgeRail", geometry: true, visible: true, collider: true},
                #{label: "inner edge rail", path: ramp_path + "/EdgeRailInner", geometry: true, visible: true, collider: true},
            ],
            parameters: [
                #{name: "ramp mass", attr: "physics:mass", positive: true},
                #{name: "deployment command", attr: "lunco:deployment_angle_rad", max_abs: max_ramp_command_rad()},
                #{name: "mechanism description", attr: "lunco:mechanism", nonempty: true},
            ],
            parameter_status: "command envelope is a control surrogate; mechanical limit TBD",
        });
    }

    let tanks = [];
    for name in ["TankPX", "TankNX", "TankPZ", "TankNZ"] {
        tanks.push(#{
            id: "propellant_" + name,
            label: "propellant tank " + name,
            rule: "GR-033",
            path: root + "/" + name,
            attrs: ["visibility", "radius", "height", "axis"],
            expected_type: "Capsule",
            geometry: true,
            visible: true,
            collider: false,
            parameter_status: "four-tank public integration fact; exact tank dimensions TBD",
        });
    }

    [
        #{
            id: "lander",
            label: "Griffin lander root",
            rule: "GR-001",
            path: root,
            attrs: ["visibility", "lunco:payload_class_kg", "lunco:payload_class_status",
                    "lunco:configuration", "lunco:parameter_status",
                    "lunco:provenance:source", "lunco:collision:contract"],
            expected_type: "Xform",
            required_children: [root + "/BoxyAirframe",
                root + "/IsogridDeck", root + "/PayloadAdapter",
                root + "/MainPropulsion", root + "/SolarPanelPort",
                root + "/SolarPanelStarboard", root + "/Nozzle",
                root + "/TankPX", root + "/TankNX", root + "/TankPZ",
                root + "/TankNZ"],
            parameters: [
                #{name: "payload capacity", attr: "lunco:payload_class_kg", exact: max_payload_kg()},
                #{name: "payload provenance status", attr: "lunco:payload_class_status", contains: "not a public"},
                #{name: "configuration", attr: "lunco:configuration", contains: "four legs"},
                #{name: "parameter status", attr: "lunco:parameter_status", nonempty: true},
                #{name: "source provenance", attr: "lunco:provenance:source", nonempty: true},
                #{name: "collision contract", attr: "lunco:collision:contract", nonempty: true},
            ],
            parameter_status: "project topology and payload class; flight ICD TBD",
        },
        #{
            id: "deck",
            label: "payload deck",
            rule: "GR-001",
            path: root + "/IsogridDeck",
            attrs: ["visibility"],
            expected_type: "Xform",
            visible: true,
            required_children: [root + "/IsogridDeck/DeckPlate"],
            geometry_children: [#{label: "deck plate", path: root + "/IsogridDeck/DeckPlate", geometry: true, collider: true}],
            parameter_status: "prototype deck dimensions; flight ICD TBD",
        },
        #{
            id: "boxy_airframe",
            label: "boxy primary airframe",
            rule: "GR-018",
            path: root + "/BoxyAirframe",
            attrs: ["visibility", "physics:collisionEnabled"],
            expected_type: "Cube",
            geometry: true,
            visible: true,
            collider: true,
            parameter_status: "public references establish a boxy frame; exact dimensions and mass are study values",
        },
        #{
            id: "payload_deck_plate",
            label: "rectangular isogrid deck plate",
            rule: "GR-001",
            path: root + "/IsogridDeck/DeckPlate",
            attrs: ["visibility", "physics:collisionEnabled"],
            expected_type: "Cube",
            geometry: true,
            visible: true,
            collider: true,
            parameter_status: "public isogrid deck requirement; bolt pattern and exact dimensions TBD",
        },
        #{
            id: "payload_adapter",
            label: "payload adapter",
            rule: "GR-002",
            path: root + "/PayloadAdapter",
            attrs: [],
            expected_type: "Xform",
            required_children: [root + "/PayloadAdapter/AdapterPlate"],
            geometry_children: [#{label: "adapter plate", path: root + "/PayloadAdapter/AdapterPlate", geometry: true, collider: true}],
            parameter_status: "FLIP interface geometry; release datum TBD",
        },
        #{
            id: "main_propulsion",
            label: "main propulsion interface",
            rule: "GR-006",
            path: root + "/MainPropulsion",
            attrs: [],
            expected_type: "Scope",
            required_children: [root + "/MainPropulsion/MainChamber",
                root + "/MainPropulsion/FuelTank", root + "/MainPropulsion/OxidizerTank"],
            parameter_status: "thrust, propellant, dry mass, and nozzle ICD TBD",
        },
        #{
            id: "solar_port",
            label: "port solar array",
            rule: "GR-006",
            path: root + "/SolarPanelPort",
            attrs: ["visibility", "lunco:installation"],
            expected_type: "Xform",
            visible: true,
            required_children: [root + "/SolarPanelPort/Cells"],
            geometry_children: [#{label: "solar cells", path: root + "/SolarPanelPort/Cells", geometry: true, collider: false}],
            parameters: [#{name: "installation", attr: "lunco:installation", nonempty: true}],
            parameter_status: "visual/power proxy; electrical ICD TBD",
        },
        #{
            id: "solar_starboard",
            label: "starboard solar array",
            rule: "GR-006",
            path: root + "/SolarPanelStarboard",
            attrs: ["visibility", "lunco:installation"],
            expected_type: "Xform",
            visible: true,
            required_children: [root + "/SolarPanelStarboard/Cells"],
            geometry_children: [#{label: "solar cells", path: root + "/SolarPanelStarboard/Cells", geometry: true, collider: false}],
            parameters: [#{name: "installation", attr: "lunco:installation", nonempty: true}],
            parameter_status: "visual/power proxy; electrical ICD TBD",
        },
        #{
            id: "nozzle",
            label: "main engine nozzle",
            rule: "GR-015",
            path: root + "/Nozzle",
            attrs: ["visibility", "lunco:parameter_status", "lunco:provenance:source"],
            expected_type: "Xform",
            visible: true,
            required_children: [root + "/Nozzle/MainEngineCluster"],
            geometry_children: [],
            parameters: [
                #{name: "nozzle parameter status", attr: "lunco:parameter_status", nonempty: true},
                #{name: "nozzle provenance", attr: "lunco:provenance:source", nonempty: true},
            ],
            parameter_status: "seven-engine public configuration; bell contour and engine spacing are study values",
        },
        #{
            id: "main_engine_cluster",
            label: "seven-engine visual cluster",
            rule: "GR-032",
            path: root + "/Nozzle/MainEngineCluster",
            attrs: ["visibility"],
            expected_type: "Xform",
            visible: true,
            required_children: [
                root + "/Nozzle/MainEngineCluster/Engine01",
                root + "/Nozzle/MainEngineCluster/Engine02",
                root + "/Nozzle/MainEngineCluster/Engine03",
                root + "/Nozzle/MainEngineCluster/Engine04",
                root + "/Nozzle/MainEngineCluster/Engine05",
                root + "/Nozzle/MainEngineCluster/Engine06",
                root + "/Nozzle/MainEngineCluster/Engine07",
            ],
            geometry_children: [],
            parameter_status: "seven main engines are public; visual bells are a study representation",
        },
    ] + tanks + legs + ramps
}

fn _part_report(doc_id, spec) {
    let findings = _audit_part(doc_id, spec);
    #{
        id: spec.id,
        label: spec.label,
        path: spec.path,
        ok: findings.len() == 0,
        findings: findings,
        parameter_status: spec.parameter_status,
    }
}

/// Return every named part contract, including its known/TBD parameter status.
/// This is useful for an Editor inspector or a future generic CAD workbench.
fn part_report(doc_id, root) {
    let parts = [];
    let findings = [];
    for spec in part_manifest(root) {
        let result = _part_report(doc_id, spec);
        parts.push(result);
        for finding in result.findings { findings.push(finding); }
    }
    #{ok: findings.len() == 0, root: root, doc_id: doc_id, parts: parts, findings: findings}
}

/// Return the executable structural rule set. Keep this list next to the
/// runtime report so a new check cannot silently exist only in a document.
fn executable_rules() {
    [
        "GR-001", "GR-002", "GR-003", "GR-005", "GR-006",
        "GR-009", "GR-010", "GR-011", "GR-012", "GR-013",
        "GR-015", "GR-016", "GR-017", "GR-018", "GR-019", "GR-020",
        "GR-029", "GR-032", "GR-033",
    ]
}

/// Checks that are documented but intentionally not part of the green
/// executable suite yet. The runtime report exposes them so a caller cannot
/// confuse a partial pass with vehicle qualification.
fn planned_rules() {
    [
        "GR-004", "GR-007", "GR-008", "GR-014", "GR-021", "GR-022",
        "GR-023", "GR-024", "GR-025", "GR-027", "GR-028",
    ]
}

fn mission_scope_rules() { ["GR-026"] }

fn process_gate_rules() { ["GR-030"] }

fn _missing_rule_ids(expected, actual) {
    let missing = [];
    for id in expected {
        if !_has(actual, id) { missing.push(id); }
    }
    missing
}

/// Compose one machine-readable report for the live runtime test. This is a
/// report rather than a second linter: `lint` remains the authoritative
/// structural implementation, while this function proves that the runtime
/// exercised every executable rule, every named part, and relational layout.
fn runtime_report(doc_id, root) {
    let lint_report = lint(doc_id, root);
    let parts = part_report(doc_id, root);
    let layout = layout_report(doc_id, root);
    let expected_rules = executable_rules();
    let errors = [];

    for error in lint_report.errors { errors.push(error); }
    for id in _missing_rule_ids(expected_rules, lint_report.checked_rules) {
        errors.push("runtime did not execute " + id);
    }
    for id in _missing_rule_ids(expected_rules, griffin_spec::check_catalog()) {
        errors.push("spec catalog is missing executable rule " + id);
    }

    if parts.parts.len() != 20 {
        errors.push("expected 20 manifest parts, got " + parts.parts.len());
    }
    let part_failures = [];
    for part in parts.parts {
        if !part.ok { part_failures.push(part.id); }
    }
    for id in part_failures {
        errors.push("part contract failed: " + id);
    }
    if !layout.ok {
        errors.push("relational layout report has " + layout.findings.len() + " finding(s)");
    }

    #{
        ok: errors.len() == 0,
        root: root,
        doc_id: doc_id,
        errors: errors,
        checked_rules: lint_report.checked_rules,
        expected_rules: expected_rules,
        planned_rules: planned_rules(),
        mission_scope_rules: mission_scope_rules(),
        process_gate_rules: process_gate_rules(),
        parts: parts.parts,
        part_failures: part_failures,
        layout: layout,
        structural: lint_report,
    }
}

fn _report(doc_id, root, findings) {
    let errors = [];
    for finding in findings {
        errors.push(finding.id + " " + finding.path + ": " + finding.message);
    }
    #{
        ok: errors.len() == 0,
        doc_id: doc_id,
        root: root,
        findings: findings,
        errors: errors,
        warnings: [],
        checked_rules: [
            "GR-001", "GR-002", "GR-003", "GR-005", "GR-006",
            "GR-009", "GR-010", "GR-011", "GR-012", "GR-013",
            "GR-015", "GR-016", "GR-017", "GR-018", "GR-019", "GR-020",
            "GR-029", "GR-032", "GR-033",
        ],
    }
}

/// Return whether a payload load is inside the authored capacity boundary.
/// This pure predicate is also used by the negative-boundary test.
fn payload_load_allowed(payload_kg, max_payload_kg) {
    payload_kg != () && max_payload_kg != ()
        && payload_kg >= 0.0 && payload_kg <= max_payload_kg
}

fn payload_load_check(payload_kg, max_payload_kg) {
    if payload_kg == () || max_payload_kg == () {
        return #{ ok: false, error: "payload mass and max payload are required" };
    }
    if payload_load_allowed(payload_kg, max_payload_kg) {
        return #{ ok: true, payload_kg: payload_kg, max_payload_kg: max_payload_kg };
    }
    #{
        ok: false,
        payload_kg: payload_kg,
        max_payload_kg: max_payload_kg,
        error: "payload load exceeds the Griffin Twin capacity boundary",
    }
}

/// Runtime commands may request a ramp angle in radians, but the authored USD
/// transform remains degrees. Keep that unit conversion at this gate so an
/// editor tool cannot silently widen the control envelope.
fn ramp_command_allowed(angle_rad) {
    angle_rad != () && angle_rad.abs() <= max_ramp_command_rad()
}

fn ramp_command_check(angle_rad) {
    if angle_rad == () {
        return #{ ok: false, error: "ramp command angle is required" };
    }
    if ramp_command_allowed(angle_rad) {
        return #{ ok: true, angle_rad: angle_rad, limit_rad: max_ramp_command_rad() };
    }
    #{
        ok: false,
        angle_rad: angle_rad,
        limit_rad: max_ramp_command_rad(),
        error: "ramp command exceeds the Griffin Twin angular limit",
    }
}

/// Lint the composed Griffin contract. `root` is an absolute composed USD
/// path, such as `/Griffin1` in the isolated Editor preview or
/// `/GriffinRequirementsTest/Griffin1` in the Twin test scene.
fn lint(doc_id, root) {
    let findings = [];
    let root_record = _query(doc_id, root, [
        "lunco:payload_class_kg",
        "lunco:payload_class_status",
        "lunco:configuration",
        "lunco:parameter_status",
        "lunco:provenance:source",
        "lunco:collision:contract",
        "lunco:egress",
    ], (), true);

    if !_ok(root_record) {
        findings.push(_finding("GR-001", root, "composed Griffin root is missing"));
        return _report(doc_id, root, findings);
    }
    if root_record.type_name != "Xform" {
        findings.push(_finding("GR-001", root,
            "expected Xform root, got " + root_record.type_name));
    }

    // Run the generic manifest audit as part of the Griffin lint. The
    // specialized checks below still cover topology relationships and exact
    // counts; the manifest covers presence, geometry, collision ownership,
    // and per-part parameters uniformly.
    let part_contract = part_report(doc_id, root);
    for finding in part_contract.findings { findings.push(finding); }
    let layout_contract = layout_report(doc_id, root);
    for finding in layout_contract.findings { findings.push(finding); }

    let attrs = root_record.attrs;
    let payload = attrs["lunco:payload_class_kg"];
    if payload == () {
        findings.push(_finding("GR-009", root, "missing lunco:payload_class_kg capacity"));
    } else if payload != max_payload_kg() {
        findings.push(_finding("GR-009", root,
            "payload capacity must be the Twin requirement 625 kg"));
    } else if !payload_load_allowed(payload, max_payload_kg()) {
        findings.push(_finding("GR-009", root, "payload capacity is outside the valid mass range"));
    }

    let payload_status = attrs["lunco:payload_class_status"];
    if payload_status == () || !payload_status.contains("not a public") {
        findings.push(_finding("GR-005", root,
            "payload capacity needs an explicit non-flight provenance status"));
    }
    let configuration = attrs["lunco:configuration"];
    if configuration == () || !configuration.contains("four legs") {
        findings.push(_finding("GR-010", root,
            "configuration must state the four-leg study topology"));
    }
    let parameter_status = attrs["lunco:parameter_status"];
    if parameter_status == () || parameter_status == "" {
        findings.push(_finding("GR-005", root,
            "missing explicit parameter status / TBD boundary"));
    }
    let provenance = attrs["lunco:provenance:source"];
    if provenance == () || provenance == "" {
        findings.push(_finding("GR-005", root, "missing source provenance"));
    }
    let collision_contract = attrs["lunco:collision:contract"];
    if collision_contract == () || collision_contract == "" {
        findings.push(_finding("GR-012", root, "missing collision contract"));
    }
    let egress = attrs["lunco:egress"];
    if egress == () || !egress.contains("adapter") || !egress.contains("ramp") {
        findings.push(_finding("GR-013", root,
            "egress contract must name the payload adapter and ramp mechanism"));
    }

    let leg_names = ["LegPX", "LegNX", "LegPZ", "LegNZ"];
    let pad_names = ["PadPX", "PadNX", "PadPZ", "PadNZ"];
    let visible_leg_count = 0;
    if root_record.children != () {
        for child in root_record.children {
            // Historical geometry remains in the wrapper for comparison, but
            // must stay hidden; only visible direct Leg* bodies count here.
            if child.contains("/Leg") && !child.contains("_Spring") {
                let child_record = _query(doc_id, child, ["visibility"], (), false);
                if _ok(child_record) && child_record.attrs["visibility"] != "invisible" {
                    visible_leg_count += 1;
                }
            }
        }
    }
    if visible_leg_count != 4 {
        findings.push(_finding("GR-010", root,
            "expected exactly four visible functional landing-leg bodies, got "
                + visible_leg_count));
    }
    for i in 0..leg_names.len() {
        let leg_name = leg_names[i];
        let leg_path = root + "/" + leg_name;
        let leg = _query(doc_id, leg_path, ["physics:mass", "visibility"], (), true);
        if !_ok(leg) {
            findings.push(_finding("GR-010", leg_path, "landing-leg body is missing"));
        } else {
            if leg.type_name != "Xform" {
                findings.push(_finding("GR-010", leg_path,
                    "landing-leg body must be an Xform"));
            }
            if leg.attrs["visibility"] == "invisible" {
                findings.push(_finding("GR-010", leg_path,
                    "functional landing-leg body is hidden"));
            }
            if leg.attrs["physics:mass"] == () || leg.attrs["physics:mass"] <= 0.0 {
                findings.push(_finding("GR-012", leg_path,
                    "landing-leg body needs a positive mass"));
            }
            let pad_path = leg_path + "/" + pad_names[i];
            let pad = _query(doc_id, pad_path, ["physics:collisionEnabled"], (), false);
            if !_ok(pad) || pad.attrs["physics:collisionEnabled"] != true {
                findings.push(_finding("GR-010", pad_path,
                    "each landing leg needs one enabled foot-pad collider"));
            }
            let strut_path = leg_path + "/Strut";
            if !_ok(_query(doc_id, strut_path, [], (), false)) {
                findings.push(_finding("GR-010", strut_path,
                    "each landing leg needs an explicit strut child"));
            }
        }
    }

    let ramp_names = ["EgressRampPort", "EgressRampStarboard"];
    let hinge_names = ["RampHingePort", "RampHingeStarboard"];
    for i in 0..ramp_names.len() {
        let ramp_path = root + "/" + ramp_names[i];
        let ramp = _query(doc_id, ramp_path,
            ["physics:mass", "lunco:mechanism", "lunco:deployment_angle_rad"], (), true);
        if !_ok(ramp) {
            findings.push(_finding("GR-003", ramp_path, "egress ramp body is missing"));
        } else {
            if ramp.type_name != "Xform" {
                findings.push(_finding("GR-003", ramp_path,
                    "egress ramp must be an Xform rigid-body assembly"));
            }
            if ramp.attrs["physics:mass"] == () || ramp.attrs["physics:mass"] <= 0.0 {
                findings.push(_finding("GR-012", ramp_path,
                    "egress ramp needs a positive mass"));
            }
            if ramp.attrs["lunco:mechanism"] == () || ramp.attrs["lunco:mechanism"] == "" {
                findings.push(_finding("GR-003", ramp_path,
                    "egress ramp needs an explicit mechanism description"));
            }
            for child_name in ["Surface", "EdgeRail", "EdgeRailInner"] {
                let child_path = ramp_path + "/" + child_name;
                let child = _query(doc_id, child_path, ["physics:collisionEnabled"], (), false);
                if !_ok(child) || child.attrs["physics:collisionEnabled"] != true {
                    findings.push(_finding("GR-003", child_path,
                        "ramp contact geometry must have collision enabled"));
                }
            }
        }

        let hinge_path = root + "/" + hinge_names[i];
        let hinge = _query(doc_id, hinge_path,
            ["physics:axis", "physics:lowerLimit", "physics:upperLimit"],
            ["physics:body0", "physics:body1"], false);
        if !_ok(hinge) {
            findings.push(_finding("GR-011", hinge_path, "ramp revolute joint is missing"));
        } else {
            if hinge.type_name != "PhysicsRevoluteJoint" {
                findings.push(_finding("GR-011", hinge_path,
                    "ramp hinge must be PhysicsRevoluteJoint"));
            }
            if _relationship(hinge, "physics:body0") != root
                || _relationship(hinge, "physics:body1") != ramp_path {
                findings.push(_finding("GR-011", hinge_path,
                    "ramp hinge must connect the Griffin root and its ramp body"));
            }
            if hinge.attrs["physics:axis"] != "Z" {
                findings.push(_finding("GR-011", hinge_path,
                    "ramp hinge axis must be the authored Z axis"));
            }
            let lower = hinge.attrs["physics:lowerLimit"];
            let upper = hinge.attrs["physics:upperLimit"];
            if lower == () || upper == () || lower >= upper {
                findings.push(_finding("GR-011", hinge_path,
                    "ramp hinge needs ordered angular limits"));
            }
        }
    }

    let deck = _query(doc_id, root + "/IsogridDeck", [], (), true);
    if !_ok(deck) || !_has(deck.children, root + "/IsogridDeck/DeckPlate") {
        findings.push(_finding("GR-001", root + "/IsogridDeck",
            "top payload deck and deck plate are required"));
    } else {
        let plate = _query(doc_id, root + "/IsogridDeck/DeckPlate",
            ["physics:collisionEnabled"], (), false);
        if !_ok(plate) || plate.attrs["physics:collisionEnabled"] != true {
            findings.push(_finding("GR-012", root + "/IsogridDeck/DeckPlate",
                "deck plate must be an enabled collider"));
        }
    }

    let adapter = _query(doc_id, root + "/PayloadAdapter", [], (), true);
    if !_ok(adapter) || !_has(adapter.children, root + "/PayloadAdapter/AdapterPlate") {
        findings.push(_finding("GR-002", root + "/PayloadAdapter",
            "payload adapter and adapter plate are required"));
    } else {
        let plate = _query(doc_id, root + "/PayloadAdapter/AdapterPlate",
            ["physics:collisionEnabled"], (), false);
        if !_ok(plate) || plate.attrs["physics:collisionEnabled"] != true {
            findings.push(_finding("GR-002", root + "/PayloadAdapter/AdapterPlate",
                "payload adapter plate must be an enabled collider"));
        }
    }

    for child_name in ["MainPropulsion", "SolarPanelPort", "SolarPanelStarboard"] {
        if !_ok(_query(doc_id, root + "/" + child_name, [], (), false)) {
            findings.push(_finding("GR-006", root + "/" + child_name,
                "required propulsion/power subassembly is missing"));
        }
    }

    _report(doc_id, root, findings)
}

/// Run the contract against an already open Editor document and print a
/// compact report for the scripting console. The document is not closed,
/// reloaded, or saved.
fn lint_live(doc_id, root) {
    if doc_id == () {
        return #{ ok: false, error: "lint_live requires an explicit USD doc_id" };
    }
    let report = lint(doc_id, root);
    print("[griffin-lint] root=" + root + " doc_id=" + doc_id
        + " result=" + (if report.ok { "PASS" } else { "FAIL" }));
    for error in report.errors { print("[griffin-lint] ERROR " + error); }
    report
}

/// Print the per-part contract, including known values and explicit TBD
/// boundaries, against an already open Editor document. This is read-only.
fn part_report_live(doc_id, root) {
    if doc_id == () {
        return #{ ok: false, error: "part_report_live requires an explicit USD doc_id" };
    }
    let report = part_report(doc_id, root);
    print("[griffin-parts] root=" + root + " doc_id=" + doc_id
        + " result=" + (if report.ok { "PASS" } else { "FAIL" })
        + " parts=" + report.parts.len());
    for part in report.parts {
        print("[griffin-parts] " + part.id + " "
            + (if part.ok { "PASS" } else { "FAIL" })
            + " status=" + part.parameter_status);
        for finding in part.findings {
            print("[griffin-parts] ERROR " + finding.id + " "
                + finding.path + ": " + finding.message);
        }
    }
    report
}

/// Print relational geometry checks for an open Editor document. This is
/// read-only and complements `part_report_live`: it checks where parts are,
/// not just whether their named prims exist.
fn layout_report_live(doc_id, root) {
    if doc_id == () {
        return #{ ok: false, error: "layout_report_live requires an explicit USD doc_id" };
    }
    let report = layout_report(doc_id, root);
    print("[griffin-layout] root=" + root + " doc_id=" + doc_id
        + " result=" + (if report.ok { "PASS" } else { "FAIL" })
        + " findings=" + report.findings.len());
    for finding in report.findings {
        print("[griffin-layout] ERROR " + finding.id + " "
            + finding.path + ": " + finding.message);
    }
    report
}

/// Read the live model's authored capacity and gate a proposed payload load.
/// It is intentionally read-only: the payload mass belongs to the composed
/// payload/mission instance, while the capacity belongs to the Griffin asset.
fn payload_limit_live(doc_id, root, payload_kg) {
    let record = _query(doc_id, root, ["lunco:payload_class_kg"], (), false);
    if !_ok(record) || record.attrs["lunco:payload_class_kg"] == () {
        return #{ ok: false, error: "live Griffin capacity is missing" };
    }
    let result = payload_load_check(payload_kg, record.attrs["lunco:payload_class_kg"]);
    print("[griffin-lint] payload=" + payload_kg + "kg max="
        + record.attrs["lunco:payload_class_kg"] + "kg result="
        + (if result.ok { "PASS" } else { "FAIL" }));
    result
}

/// Apply one command-limited ramp deployment edit to an open Editor document.
/// This is a live typed edit, not a file rewrite. The caller must re-run
/// `lint_live` after the deferred projection reports the new generation.
fn apply_ramp_deployment(doc_id, edit_target, root, ramp_path, angle_rad, parent_gen) {
    if doc_id == () || edit_target == () || root == () || ramp_path == () {
        return #{ ok: false, error: "live ramp edit requires doc_id, edit_target, root, and ramp_path" };
    }
    if ramp_path != root + "/EgressRampPort"
        && ramp_path != root + "/EgressRampStarboard" {
        return #{ ok: false, error: "ramp_path is not one of the two Griffin egress ramps" };
    }
    let limit = ramp_command_check(angle_rad);
    if !limit.ok { return limit; }

    let before = lint(doc_id, root);
    if !before.ok {
        return #{ ok: false, error: "refusing edit: Griffin contract already fails", report: before };
    }

    let angle_deg = angle_rad * 180.0 / pi_value();
    let result = cmd("ApplyUsdOps", #{
        doc_id: doc_id,
        parent_gen: parent_gen,
        label: "Griffin command-limited ramp deployment",
        ops: [
            #{ SetRotate: #{
                edit_target: edit_target,
                path: ramp_path,
                value: [0.0, 0.0, angle_deg],
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: ramp_path,
                name: "lunco:deployment_angle_rad",
                type_name: "float",
                value: angle_rad.to_string(),
            }},
        ],
    });
    #{
        ok: result.ok == true,
        accepted: result.ok == true,
        edit: result,
        validation_pending: result.ok == true,
        next: "after projection, call griffin_requirements::lint_live(doc_id, root)",
    }
}
