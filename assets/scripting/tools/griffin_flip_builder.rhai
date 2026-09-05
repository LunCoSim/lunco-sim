// Dynamic, mission-level assembly recipes for the Griffin lander and FLIP
// rover study assets.
//
// These functions compose the generic assembly_builder plans. They own only
// the authored Griffin/FLIP construction policy: paired ramp identities and
// wheel-station layout. USD still owns the exact paths, dimensions, masses,
// frames, and component metadata supplied by the caller; no flight value is
// invented here and no second USD writer is introduced.

fn plan_error(message) {
    #{ ok: false, error: message, ops: [] }
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn close(actual, expected, epsilon) {
    actual != () && expected != () && (actual - expected).abs() <= epsilon
}

/// Build both Griffin egress ramps as one symmetric, reviewed plan.
///
/// Each entry in `ramps` is an explicit map containing `side` ("port" or
/// "starboard"), `body_path`, `joint_name`, `collider_paths`, `mass`,
/// `center_of_mass`, `diagonal_inertia`, and the two joint frame positions and
/// rotations. The common lander body, joint parent, axis, limits, and collision
/// policy are explicit arguments. The recipe rejects missing sides, duplicate
/// identities, and a ramp accidentally pointing back at the lander body.
fn griffin_ramp_pair_plan(doc, edit_target, lander_path, joint_parent, axis,
    lower_limit, upper_limit, collision_enabled, ramps) {
    if doc == () || edit_target == () || lander_path == () || joint_parent == ()
        || ramps == () || ramps.len() != 2 {
        return plan_error("griffin_ramp_pair_plan requires two explicit ramp specifications");
    }

    let sides = [];
    let bodies = [];
    let joints = [];
    let parts = [];
    let ops = [];
    for ramp in ramps {
        if ramp == () || ramp.side == () || ramp.body_path == ()
            || ramp.joint_name == () || ramp.collider_paths == ()
            || ramp.mass == () || ramp.center_of_mass == ()
            || ramp.diagonal_inertia == () || ramp.local_pos0 == ()
            || ramp.local_pos1 == () || ramp.local_rot0 == ()
            || ramp.local_rot1 == () {
            return plan_error("Griffin ramp requires side, body, joint, colliders, mass, inertia, and both frames");
        }
        if ramp.side != "port" && ramp.side != "starboard" {
            return plan_error("Griffin ramp side must be port or starboard");
        }
        if sides.contains(ramp.side) {
            return plan_error("Griffin ramp sides must be unique");
        }
        if ramp.body_path == lander_path || bodies.contains(ramp.body_path) {
            return plan_error("Griffin ramp body paths must be distinct from the lander and each other");
        }
        if joints.contains(ramp.joint_name) {
            return plan_error("Griffin ramp joint names must be unique");
        }

        let part = assembly_builder::existing_hinged_body_plan(
            doc, edit_target, ramp.body_path, ramp.mass,
            ramp.center_of_mass, ramp.diagonal_inertia, ramp.collider_paths,
            joint_parent, ramp.joint_name, lander_path, axis,
            ramp.local_pos0, ramp.local_pos1, ramp.local_rot0, ramp.local_rot1,
            lower_limit, upper_limit, collision_enabled,
        );
        if !part.ok { return part; }
        sides.push(ramp.side);
        bodies.push(ramp.body_path);
        joints.push(ramp.joint_name);
        parts.push(#{ side: ramp.side, body: ramp.body_path, joint: part.joint });
        for op in part.ops { ops.push(op); }
    }

    if !sides.contains("port") || !sides.contains("starboard") {
        return plan_error("Griffin ramp pair requires one port and one starboard ramp");
    }
    #{
        ok: true,
        kind: "griffin_ramp_pair",
        lander: lander_path,
        parts: parts,
        ops: ops,
    }
}

/// Build a validated four-wheel FLIP station layout plan.
///
/// Each station is `{path, wheel, index, translation, radius, width}`. The
/// exact station and wheel prims must already compose. The wheel's authored
/// vehicle index/radius/width are checked against the manifest before one
/// local station translation per wheel is emitted. This catches swapped
/// stations and geometry mismatches before a proposal reaches USD.
fn flip_four_wheel_layout_plan(doc, edit_target, root_path, stations) {
    if doc == () || edit_target == () || root_path == ()
        || stations == () || stations.len() != 4 {
        return plan_error("flip_four_wheel_layout_plan requires exactly four wheel stations");
    }

    let indexes = [];
    let station_paths = [];
    let wheel_paths = [];
    let ops = [];
    let layout = [];
    for station in stations {
        if station.path == () || station.wheel == () || station.translation == ()
            || station.index == () || station.radius == () || station.width == () {
            return plan_error("FLIP wheel station requires path, wheel, index, translation, radius, and width");
        }
        if !station.path.starts_with(root_path + "/")
            || !station.wheel.starts_with(station.path + "/") {
            return plan_error("FLIP wheel station and wheel must be exact descendants of the rover root");
        }
        if station_paths.contains(station.path) || wheel_paths.contains(station.wheel) {
            return plan_error("FLIP wheel station identities must be unique");
        }
        if station.index < 0 || station.index > 3 || indexes.contains(station.index) {
            return plan_error("FLIP wheel attachment indexes must be the unique values 0, 1, 2, and 3");
        }
        if station.translation.len() != 3 || station.radius <= 0.0 || station.width <= 0.0 {
            return plan_error("FLIP wheel station has invalid placement or wheel dimensions");
        }

        let station_prim = query("QueryUsdPrim", #{
            doc: doc,
            path: station.path,
            children: false,
        });
        if !query_ok(station_prim) || station_prim.type_name != "Xform" {
            return plan_error(station.path + ": FLIP wheel station must be an existing Xform");
        }
        let wheel = query("QueryUsdPrim", #{
            doc: doc,
            path: station.wheel,
            attrs: [
                "physxVehicleWheelAttachment:index",
                "physxVehicleWheel:radius",
                "physxVehicleWheel:width",
            ],
        });
        if !query_ok(wheel) || wheel.type_name != "Cylinder" || wheel.attrs == () {
            return plan_error(station.wheel + ": FLIP wheel must be an existing Cylinder");
        }
        if wheel.attrs["physxVehicleWheelAttachment:index"] != station.index {
            return plan_error(station.wheel + ": authored wheel index does not match the layout manifest");
        }
        if !close(wheel.attrs["physxVehicleWheel:radius"], station.radius, 0.0001)
            || !close(wheel.attrs["physxVehicleWheel:width"], station.width, 0.0001) {
            return plan_error(station.wheel + ": authored wheel dimensions do not match the layout manifest");
        }

        let placement = assembly_builder::place_plan(
            edit_target, station.path, station.translation, (), (),
        );
        if !placement.ok { return placement; }
        for op in placement.ops { ops.push(op); }
        indexes.push(station.index);
        station_paths.push(station.path);
        wheel_paths.push(station.wheel);
        layout.push(#{
            path: station.path,
            wheel: station.wheel,
            index: station.index,
            translation: station.translation,
        });
    }

    for index in 0..4 {
        if !indexes.contains(index) {
            return plan_error("FLIP wheel layout is missing attachment index " + index);
        }
    }
    #{
        ok: true,
        kind: "flip_four_wheel_layout",
        root: root_path,
        stations: layout,
        ops: ops,
    }
}
