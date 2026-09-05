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
        if !ramp.body_path.starts_with(lander_path + "/") {
            return plan_error("Griffin ramp bodies must be explicit descendants of the lander");
        }
        if ramp.body_path == lander_path || bodies.contains(ramp.body_path) {
            return plan_error("Griffin ramp body paths must be distinct from the lander and each other");
        }
        if joints.contains(ramp.joint_name) {
            return plan_error("Griffin ramp joint names must be unique");
        }
        let joint_path = joint_parent + "/" + ramp.joint_name;
        let existing_joint = query("QueryUsdPrim", #{
            doc: doc,
            path: joint_path,
            children: false,
        });
        if query_ok(existing_joint) && existing_joint.type_name != "Cube" {
            return plan_error(joint_path + ": joint path is already occupied by "
                + existing_joint.type_name + "; update the existing joint or choose a new identity");
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

fn relationship_target(record, name) {
    if !query_ok(record) || record.relationships == ()
        || record.relationships[name] == () || record.relationships[name].len() != 1 {
        return ();
    }
    record.relationships[name][0]
}

/// Build one reviewed Griffin/FLIP construction plan from an explicit
/// assembly manifest. The manifest supplies all project-specific values and
/// paths; this recipe only composes the reusable ramp and wheel planners and
/// verifies that the existing payload adapter connects the intended bodies.
///
/// The returned operations are still pure typed plans. Callers can inspect
/// and propose them as one change set, while the adapter contract is checked
/// before any operation reaches USD.
fn griffin_mission_plan(doc, edit_target, manifest) {
    if doc == () || edit_target == () || manifest == ()
        || manifest.root_path == () || manifest.lander_path == ()
        || manifest.flip_path == () || manifest.adapter_joint_path == ()
        || manifest.ramps == () || manifest.stations == ()
        || manifest.ramp_axis == () || manifest.lower_limit == ()
        || manifest.upper_limit == () || manifest.collision_enabled == () {
        return plan_error("griffin_mission_plan requires root, lander, FLIP, adapter, ramp, and wheel manifest fields");
    }

    let root = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.root_path,
        children: true,
    });
    let lander = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.lander_path,
        children: false,
    });
    let flip = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.flip_path,
        children: false,
    });
    if !query_ok(root) || root.type_name != "Xform"
        || !query_ok(lander) || lander.type_name != "Xform"
        || !query_ok(flip) || flip.type_name != "Xform" {
        return plan_error("Griffin mission manifest must name existing Xform root, lander, and FLIP prims");
    }
    if root.children == () || !root.children.contains(manifest.lander_path)
        || !root.children.contains(manifest.flip_path) {
        return plan_error("Griffin mission lander and FLIP must be direct children of the declared root");
    }

    let adapter = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.adapter_joint_path,
        attrs: [
            "physics:localPos0", "physics:localPos1",
            "physics:localRot0", "physics:localRot1",
        ],
        rels: ["physics:body0", "physics:body1"],
    });
    let body0 = relationship_target(adapter, "physics:body0");
    let body1 = relationship_target(adapter, "physics:body1");
    if !query_ok(adapter) || adapter.type_name != "PhysicsFixedJoint"
        || body0 != manifest.lander_path || body1 != manifest.flip_path
        || adapter.attrs == () || adapter.attrs["physics:localPos0"] == ()
        || adapter.attrs["physics:localPos1"] == () {
        return plan_error("Griffin mission adapter must be an explicit fixed joint between the declared lander and FLIP");
    }

    let ramp_plan = griffin_ramp_pair_plan(
        doc, edit_target, manifest.lander_path, manifest.lander_path,
        manifest.ramp_axis, manifest.lower_limit, manifest.upper_limit,
        manifest.collision_enabled, manifest.ramps,
    );
    if !ramp_plan.ok { return ramp_plan; }
    let wheel_plan = flip_four_wheel_layout_plan(
        doc, edit_target, manifest.flip_path, manifest.stations,
    );
    if !wheel_plan.ok { return wheel_plan; }

    let ops = [];
    for op in ramp_plan.ops { ops.push(op); }
    for op in wheel_plan.ops { ops.push(op); }
    #{
        ok: true,
        kind: "griffin_mission",
        root: manifest.root_path,
        lander: manifest.lander_path,
        flip: manifest.flip_path,
        adapter: #{
            path: manifest.adapter_joint_path,
            body0: body0,
            body1: body1,
            type_name: adapter.type_name,
        },
        ramps: ramp_plan.parts,
        stations: wheel_plan.stations,
        ops: ops,
    }
}

fn vector_distance(a, b) {
    if a == () || b == () || a.len() != 3 || b.len() != 3 { return (); }
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn mission_parent_path(path) {
    let parts = path.split("/");
    if parts.len() <= 2 { return "/"; }
    let parent = "";
    for i in 1..(parts.len() - 1) {
        if parent == "" {
            parent = "/" + parts[i];
        } else {
            parent += "/" + parts[i];
        }
    }
    parent
}

/// Build the first materialized Griffin/FLIP mission assembly from maintained
/// vessel references. This is intentionally a two-body construction plan:
/// the reference arcs remain the visual/physics asset owners, while this
/// mission recipe owns only instance identity and placement. The adapter joint
/// is deliberately a second-phase plan: first-use reference loading is
/// asynchronous, so a relationship to two newly introduced bodies must not be
/// projected before both reference closures are materialized.
///
/// The recipe rejects the old synthetic/incorrect asset identities, coincident
/// body roots, duplicate wheel paths, and occupied output identities before it
/// returns any operation. The generic USD owner still validates the ordered
/// reference, relationship, transform, journal, and projection operations.
fn griffin_mission_assembly_plan(doc, edit_target, manifest) {
    if doc == () || edit_target == () || manifest == ()
        || manifest.root_path == () || manifest.lander_path == ()
        || manifest.flip_path == () || manifest.adapter_path == ()
        || manifest.lander_asset == () || manifest.flip_asset == ()
        || manifest.lander_translation == () || manifest.lander_rotation == ()
        || manifest.flip_translation == () || manifest.flip_rotation == ()
        || manifest.adapter_local_pos0 == () || manifest.adapter_local_pos1 == ()
        || manifest.adapter_local_rot0 == () || manifest.adapter_local_rot1 == ()
        || manifest.wheel_paths == () || manifest.minimum_separation == () {
        return plan_error("griffin_mission_assembly_plan requires explicit vessel, adapter, placement, and wheel manifest fields");
    }
    if !manifest.root_path.starts_with("/") || manifest.root_path.ends_with("/")
        || !manifest.lander_path.starts_with(manifest.root_path + "/")
        || !manifest.flip_path.starts_with(manifest.root_path + "/")
        || !manifest.adapter_path.starts_with(manifest.root_path + "/") {
        return plan_error("Griffin mission assembly paths must be absolute descendants of the declared root");
    }
    if mission_parent_path(manifest.lander_path) != manifest.root_path
        || mission_parent_path(manifest.flip_path) != manifest.root_path
        || mission_parent_path(manifest.adapter_path) != manifest.root_path
        || manifest.lander_path == manifest.flip_path
        || manifest.lander_path == manifest.adapter_path
        || manifest.flip_path == manifest.adapter_path {
        return plan_error("Griffin mission lander, FLIP, and adapter must be distinct direct children of the root");
    }
    if manifest.lander_asset != "lunco://vessels/landers/descent_lander.usda"
        || manifest.flip_asset != "lunco://vessels/rovers/skid_rover.usda" {
        return plan_error("Griffin mission assembly requires the maintained descent_lander and four-wheel skid_rover asset contracts");
    }
    if manifest.wheel_paths.len() != 4 {
        return plan_error("Griffin mission assembly requires exactly four FLIP wheel paths");
    }
    if manifest.minimum_separation < 0.001 {
        return plan_error("Griffin mission assembly minimum separation must be positive");
    }
    let separation = vector_distance(manifest.lander_translation, manifest.flip_translation);
    if separation == () || separation < manifest.minimum_separation {
        return plan_error("Griffin mission lander and FLIP roots violate the explicit minimum separation");
    }

    let root = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.root_path,
        children: true,
    });
    if !query_ok(root) || root.type_name != "Xform" {
        return plan_error(manifest.root_path + ": Griffin mission assembly root is unavailable or is not an Xform");
    }
    for path in [manifest.lander_path, manifest.flip_path, manifest.adapter_path] {
        let existing = query("QueryUsdPrim", #{
            doc: doc,
            path: path,
            children: false,
        });
        if query_ok(existing) {
            return plan_error(path + ": Griffin mission output identity is already occupied");
        }
    }

    let lander_name_parts = manifest.lander_path.split("/");
    let flip_name_parts = manifest.flip_path.split("/");
    let adapter_name_parts = manifest.adapter_path.split("/");
    let lander_name = lander_name_parts[lander_name_parts.len() - 1];
    let flip_name = flip_name_parts[flip_name_parts.len() - 1];
    let adapter_name = adapter_name_parts[adapter_name_parts.len() - 1];
    let lander = assembly_builder::referenced_instance_plan(
        doc, edit_target, manifest.root_path, lander_name, "Xform",
        manifest.lander_asset, manifest.lander_translation,
        manifest.lander_rotation, (), (), (),
    );
    if !lander.ok { return lander; }
    let flip = assembly_builder::flip_rover_instance_plan(
        doc, edit_target, manifest.root_path, flip_name, manifest.flip_asset,
        manifest.flip_translation, manifest.flip_rotation, manifest.wheel_paths,
    );
    if !flip.ok { return flip; }
    let ops = [];
    for op in lander.ops { ops.push(op); }
    for op in flip.ops { ops.push(op); }
    #{
        ok: true,
        kind: "griffin_mission_assembly",
        root: manifest.root_path,
        lander: manifest.lander_path,
        flip: manifest.flip_path,
        adapter: manifest.adapter_path,
        adapter_name: adapter_name,
        lander_asset: manifest.lander_asset,
        flip_asset: manifest.flip_asset,
        wheel_paths: manifest.wheel_paths,
        separation: separation,
        ops: ops,
    }
}

/// Build the adapter after the lander and FLIP reference closures are
/// observable on the composed stage. Keeping this plan separate from the
/// first-use reference plan is a lifecycle requirement, not an authoring
/// workaround: the USD owner cannot validate a body relationship against an
/// asset whose closure is still pending.
fn griffin_mission_adapter_plan(doc, edit_target, manifest) {
    if doc == () || edit_target == () || manifest == ()
        || manifest.root_path == () || manifest.lander_path == ()
        || manifest.flip_path == () || manifest.adapter_path == ()
        || manifest.adapter_local_pos0 == () || manifest.adapter_local_pos1 == ()
        || manifest.adapter_local_rot0 == () || manifest.adapter_local_rot1 == () {
        return plan_error("griffin_mission_adapter_plan requires explicit composed bodies and joint frames");
    }
    if mission_parent_path(manifest.adapter_path) != manifest.root_path
        || manifest.adapter_path == manifest.lander_path
        || manifest.adapter_path == manifest.flip_path {
        return plan_error("Griffin mission adapter must be a distinct direct child of the mission root");
    }
    let lander = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.lander_path,
        children: true,
    });
    let flip = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.flip_path,
        children: true,
    });
    if !query_ok(lander) || lander.type_name != "Xform"
        || lander.children == () || lander.children.len() == 0
        || !query_ok(flip) || flip.type_name != "Xform"
        || flip.children == () || flip.children.len() == 0 {
        return plan_error("Griffin mission adapter requires materialized lander and FLIP reference bodies");
    }
    let existing = query("QueryUsdPrim", #{
        doc: doc,
        path: manifest.adapter_path,
        children: false,
    });
    if query_ok(existing) {
        return plan_error(manifest.adapter_path + ": Griffin mission adapter identity is already occupied");
    }
    let parts = manifest.adapter_path.split("/");
    let name = parts[parts.len() - 1];
    let joint = assembly_edit::fixed_joint_plan(
        edit_target, manifest.root_path, name,
        manifest.lander_path, manifest.flip_path,
        manifest.adapter_local_pos0, manifest.adapter_local_pos1,
        manifest.adapter_local_rot0, manifest.adapter_local_rot1, false,
    );
    if !joint.ok { return joint; }
    #{
        ok: true,
        kind: "griffin_mission_adapter",
        path: manifest.adapter_path,
        body0: manifest.lander_path,
        body1: manifest.flip_path,
        ops: joint.ops,
    }
}
