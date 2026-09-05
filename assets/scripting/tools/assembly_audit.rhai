// Composed USD geometry audits for authored assemblies.
//
// These checks deliberately operate on the composed stage through the normal
// query/world-position surface. They do not infer design intent: the caller
// supplies the mating envelope that the asset contract requires. That keeps
// this library reusable without adding a custom USD schema or a Rust-side
// geometry policy.
// Every stage-reading helper takes an explicit document id first; () selects
// the mounted live scene. Stale document projections fail the query.

const EPSILON = 0.001;

fn close_scalar(actual, expected, epsilon) {
    actual != () && expected != () && (actual - expected).abs() <= epsilon
}

fn close_vec(actual, expected, epsilon) {
    if actual == () || expected == () || actual.len() != expected.len() {
        return false;
    }
    for i in 0..actual.len() {
        if (actual[i] - expected[i]).abs() > epsilon {
            return false;
        }
    }
    true
}

fn composed_transform_contract(doc, path, expected_scale, expected_order, epsilon) {
    let record = query_prim(doc, path, ["xformOp:scale", "xformOpOrder"], (), false);
    if record == () || record.ok == false || record.attrs == () {
        return false;
    }
    close_vec(record.attrs["xformOp:scale"], expected_scale, epsilon)
        && record.attrs["xformOpOrder"] == expected_order
}

/// Return the half extents of an axis-aligned UsdGeomCube in composed metres.
/// Only translation plus scale is accepted; rotated geometry needs an explicit
/// world-space contract instead of a guessed AABB.
fn axis_aligned_cube_half_extents(doc, path) {
    let record = query_prim(doc, path, ["size", "xformOp:scale", "xformOpOrder"], (), false);
    if record == () || record.ok == false || record.attrs == () {
        return ();
    }
    let size = record.attrs["size"];
    let scale = record.attrs["xformOp:scale"];
    let order = record.attrs["xformOpOrder"];
    if size == () || scale == () || order == () || order.len() == 0 {
        return ();
    }

    let has_scale = false;
    for op in order {
        if op == "xformOp:scale" {
            has_scale = true;
        } else if op != "xformOp:translate" {
            return ();
        }
    }
    if !has_scale || scale.len() != 3 {
        return ();
    }
    [
        0.5 * size * scale[0].abs(),
        0.5 * size * scale[1].abs(),
        0.5 * size * scale[2].abs(),
    ]
}

fn world_position(doc, path) {
    let record = query_prim(doc, path, [], (), false);
    if !query_ok(record) { return (); }
    record.world_position
}

/// Test two composed prim centres against supplied world-space half extents.
/// Extents are [X, Y, Z] metres and the primitive transforms must be
/// axis-aligned in the shared stage frame.
fn world_aabb_overlap(doc, path_a, extents_a, path_b, extents_b, epsilon) {
    let a = world_position(doc, path_a);
    let b = world_position(doc, path_b);
    if a == () || b == () || extents_a == () || extents_b == ()
        || a.len() != 3 || b.len() != 3 || extents_a.len() != 3 || extents_b.len() != 3 {
        return false;
    }
    for i in 0..3 {
        if (a[i] - b[i]).abs() > extents_a[i] + extents_b[i] + epsilon {
            return false;
        }
    }
    true
}

/// Verify a host-to-mount-to-pivot seating chain in the composed stage.
/// `mount_extents` and `pivot_extents` are the authored contact envelopes,
/// not guessed dimensions hidden in the tool.
fn seat_pair(doc, host_path, host_extents, mount_path, mount_extents, pivot_path, pivot_extents, epsilon) {
    world_aabb_overlap(doc, host_path, host_extents, mount_path, mount_extents, epsilon)
        && world_aabb_overlap(doc, mount_path, mount_extents, pivot_path, pivot_extents, epsilon)
}

/// Verify every pair in a list of seven-element pair records. Each record is
/// [host_path, host_extents, mount_path, mount_extents, pivot_path,
/// pivot_extents, epsilon].
fn seat_pairs(doc, pairs) {
    for pair in pairs {
        if pair.len() != 7 || !seat_pair(doc,
            pair[0], pair[1], pair[2], pair[3], pair[4], pair[5], pair[6]
        ) {
            return false;
        }
    }
    true
}

fn query_prim(doc, path, attrs, rels, children) {
    let request = #{ path: path };
    if doc != () { request.doc = doc; }
    if attrs != () { request.attrs = attrs; }
    if rels != () { request.rels = rels; }
    if children != () { request.children = children; }
    query("QueryUsdPrim", request)
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn array_has(values, expected) {
    if values == () { return false; }
    for value in values {
        if value == expected { return true; }
    }
    false
}

fn relationship_targets(record, name) {
    if !query_ok(record) || record.relationships == () || record.relationships[name] == () {
        return [];
    }
    record.relationships[name]
}

fn required_targets(actual, expected) {
    if expected == () { return true; }
    if actual == () || actual.len() != expected.len() { return false; }
    for target in expected {
        if !array_has(actual, target) { return false; }
    }
    true
}

/// Inspect explicit composed prim paths and verify only caller-supplied
/// topology expectations. `nodes` contains `{path, expected_type?, children?,
/// relations?}`; each relation is `{name, targets?}`. This is intentionally a
/// report rather than a traversal: the caller owns the assembly manifest and
/// missing metadata is visible instead of being inferred from names.
fn topology_report(doc, nodes) {
    let records = [];
    let errors = [];
    for spec in nodes {
        let relation_names = [];
        if spec.relations != () {
            for relation in spec.relations {
                relation_names.push(relation.name);
            }
        }
        let record = query_prim(doc, spec.path, [], relation_names, true);
        if !query_ok(record) {
            errors.push(spec.path + ": composed prim is missing");
            continue;
        }
        let entry = #{
            path: spec.path,
            type_name: record.type_name,
            children: record.children,
            relationships: record.relationships,
        };
        if spec.expected_type != () && record.type_name != spec.expected_type {
            errors.push(spec.path + ": expected type " + spec.expected_type
                + ", got " + record.type_name);
        }
        if spec.children != () {
            for child in spec.children {
                if !array_has(record.children, child) {
                    errors.push(spec.path + ": missing child " + child);
                }
            }
        }
        if spec.relations != () {
            for relation in spec.relations {
                let actual = relationship_targets(record, relation.name);
                if relation.targets != () && !required_targets(actual, relation.targets) {
                    errors.push(spec.path + ": relationship " + relation.name
                        + " has unexpected targets");
                }
            }
        }
        records.push(entry);
    }
    #{ ok: errors.len() == 0, records: records, errors: errors }
}

/// Verify the reciprocal metadata for an explicit mount. This checks the
/// host socket list, socket occupancy, component attachment joint, asset and
/// joint kind; it never selects a socket or joint from a name convention.
fn mount_contract_report(doc, mounts) {
    let connections = [];
    let errors = [];
    for spec in mounts {
        let host = query_prim(doc, spec.host, (), ["lunco:mount:sockets"], false);
        let socket = query_prim(doc, spec.socket, [
            "lunco:mount:socket", "lunco:mount:joint", "lunco:mount:asset"
        ], ["lunco:mount:part"], false);
        let component = query_prim(doc, spec.component, (), ["lunco:mount:attachmentJoint"], false);
        let joint = query_prim(doc, spec.joint, (), ["physics:body0", "physics:body1"], false);
        let host_sockets = relationship_targets(host, "lunco:mount:sockets");
        let socket_part = relationship_targets(socket, "lunco:mount:part");
        let component_joint = relationship_targets(component, "lunco:mount:attachmentJoint");
        let entry = #{
            host: spec.host,
            socket: spec.socket,
            component: spec.component,
            joint: spec.joint,
            host_sockets: host_sockets,
            socket_part: socket_part,
            component_joint: component_joint,
        };
        if !query_ok(host) || !query_ok(socket) || !query_ok(component) || !query_ok(joint) {
            errors.push(spec.component + ": mount prim or joint is missing");
        } else {
            if !array_has(host_sockets, spec.socket) {
                errors.push(spec.socket + ": socket is not advertised by its host");
            }
            if !required_targets(socket_part, [spec.component]) {
                errors.push(spec.socket + ": occupancy does not point to the component");
            }
            if !required_targets(component_joint, [spec.joint]) {
                errors.push(spec.component + ": attachment joint relationship is wrong");
            }
            if spec.socket_kind != ()
                && socket.attrs["lunco:mount:socket"] != spec.socket_kind {
                errors.push(spec.socket + ": socket kind does not match the contract");
            }
            if spec.joint_kind != ()
                && socket.attrs["lunco:mount:joint"] != spec.joint_kind {
                errors.push(spec.socket + ": joint kind does not match the contract");
            }
            if spec.asset != () && socket.attrs["lunco:mount:asset"] != spec.asset {
                errors.push(spec.socket + ": asset does not match the advertised contract");
            }
            if !standard_joint_type(joint.type_name) {
                errors.push(spec.joint + ": attachment prim is not a USD joint");
            }
        }
        connections.push(entry);
    }
    #{ ok: errors.len() == 0, connections: connections, errors: errors }
}

fn cardinal_axis(axis) {
    axis == "X" || axis == "Y" || axis == "Z"
}

fn joint_has_axis(type_name) {
    type_name == "PhysicsRevoluteJoint" || type_name == "PhysicsPrismaticJoint"
        || type_name == "PhysicsSphericalJoint"
}

fn standard_joint_type(type_name) {
    joint_has_axis(type_name) || type_name == "PhysicsJoint"
        || type_name == "PhysicsFixedJoint" || type_name == "PhysicsDistanceJoint"
}

/// Check explicit joint bodies, axis and optional frame opinions. Omitted
/// primary axes on revolute, prismatic and spherical joints mean standard X.
/// Fixed, distance and generic joints have no single primary axis. Omitted
/// `physics:localPos*` values are valid when the engine's documented derived
/// anchor contract is intended; set `require_positions` only for joints whose
/// asset contract requires authored anchors.
fn joint_frame_report(doc, joints) {
    let frames = [];
    let errors = [];
    for spec in joints {
        let record = query_prim(doc, spec.path, [
            "physics:axis", "physics:localPos0", "physics:localPos1",
            "physics:localRot0", "physics:localRot1",
            "physics:lowerLimit", "physics:upperLimit"
        ], ["physics:body0", "physics:body1"], false);
        let body0 = relationship_targets(record, "physics:body0");
        let body1 = relationship_targets(record, "physics:body1");
        let entry = #{
            path: spec.path,
            type_name: if query_ok(record) { record.type_name } else { () },
            axis: if query_ok(record) { record.attrs["physics:axis"] } else { () },
            body0: body0,
            body1: body1,
        };
        if !query_ok(record) {
            errors.push(spec.path + ": joint is missing");
        } else {
            let has_axis = joint_has_axis(record.type_name);
            let axis = if has_axis && record.attrs["physics:axis"] == () {
                "X"
            } else { record.attrs["physics:axis"] };
            entry.axis = if has_axis { axis } else { () };
            if !standard_joint_type(record.type_name) {
                errors.push(spec.path + ": type is not a standard USD physics joint");
            }
            if has_axis && !cardinal_axis(axis) {
                errors.push(spec.path + ": cardinal axis is invalid");
            }
            if spec.axis != () && !has_axis {
                errors.push(spec.path + ": joint type has no primary axis");
            } else if spec.axis != () && axis != spec.axis {
                errors.push(spec.path + ": expected axis " + spec.axis + ", got " + axis);
            }
            if spec.body0 != () && !required_targets(body0, [spec.body0]) {
                errors.push(spec.path + ": body0 relationship is wrong");
            }
            if spec.body1 != () && !required_targets(body1, [spec.body1]) {
                errors.push(spec.path + ": body1 relationship is wrong");
            }
            if spec.require_positions == true
                && (record.attrs["physics:localPos0"] == ()
                    || record.attrs["physics:localPos1"] == ()) {
                errors.push(spec.path + ": required authored joint positions are missing");
            }
            if spec.require_rotations == true
                && (record.attrs["physics:localRot0"] == ()
                    || record.attrs["physics:localRot1"] == ()) {
                errors.push(spec.path + ": required authored joint rotations are missing");
            }
            let lower = record.attrs["physics:lowerLimit"];
            let upper = record.attrs["physics:upperLimit"];
            if lower != () && upper != () && lower > upper {
                errors.push(spec.path + ": lower limit exceeds upper limit");
            }
        }
        frames.push(entry);
    }
    #{ ok: errors.len() == 0, frames: frames, errors: errors }
}

/// Ensure each explicitly listed movable body has mass/inertia and every
/// listed joint connects the expected body pair. Raycast wheels are not passed
/// here: they are not rigid bodies and must not be forced into a fake joint
/// topology.
fn body_joint_coverage_report(doc, bodies) {
    let coverage = [];
    let errors = [];
    for spec in bodies {
        let body = query_prim(doc, spec.path, ["physics:mass", "physics:diagonalInertia"], (), false);
        let entry = #{ path: spec.path, joints: [], mass: () };
        if !query_ok(body) {
            errors.push(spec.path + ": rigid-body prim is missing");
        } else {
            let mass = body.attrs["physics:mass"];
            entry.mass = mass;
            if mass == () || mass <= 0.0 {
                errors.push(spec.path + ": rigid body has no positive mass");
            }
            if spec.mass != () && !close_scalar(mass, spec.mass, 0.001) {
                errors.push(spec.path + ": mass does not match the authored contract");
            }
            if spec.inertia != ()
                && !close_vec(body.attrs["physics:diagonalInertia"], spec.inertia, 0.001) {
                errors.push(spec.path + ": diagonal inertia does not match the contract");
            }
        }
        if spec.joints == () || spec.joints.len() == 0 {
            errors.push(spec.path + ": no joint coverage was declared");
        } else {
            for joint_spec in spec.joints {
                let joint = query_prim(doc, joint_spec.path, (), ["physics:body0", "physics:body1"], false);
                let body0 = relationship_targets(joint, "physics:body0");
                let body1 = relationship_targets(joint, "physics:body1");
                entry.joints.push(#{ path: joint_spec.path, body0: body0, body1: body1 });
                if !query_ok(joint) || !standard_joint_type(joint.type_name) {
                    errors.push(joint_spec.path + ": joint is missing");
                    continue;
                }
                if joint_spec.body0 != () && !required_targets(body0, [joint_spec.body0]) {
                    errors.push(joint_spec.path + ": body0 is not explicitly connected");
                }
                if joint_spec.body1 != () && !required_targets(body1, [joint_spec.body1]) {
                    errors.push(joint_spec.path + ": body1 is not explicitly connected");
                }
            }
        }
        coverage.push(entry);
    }
    #{ ok: errors.len() == 0, coverage: coverage, errors: errors }
}

/// Check authored mass/inertia values and an explicit collider list. A collider
/// may rely on a schema default, so `enabled` is optional; when supplied it
/// must match the composed `physics:collisionEnabled` value.
fn collider_mass_report(doc, bodies) {
    let reports = [];
    let errors = [];
    for spec in bodies {
        let body = query_prim(doc, spec.path, ["physics:mass", "physics:diagonalInertia"], (), false);
        let entry = #{ path: spec.path, colliders: [], mass: () };
        if !query_ok(body) {
            errors.push(spec.path + ": mass owner is missing");
        } else {
            let mass = body.attrs["physics:mass"];
            entry.mass = mass;
            if mass == () || mass <= 0.0 {
                errors.push(spec.path + ": mass must be positive");
            }
            if spec.mass != () && !close_scalar(mass, spec.mass, 0.001) {
                errors.push(spec.path + ": mass differs from expected value");
            }
            if spec.inertia != ()
                && !close_vec(body.attrs["physics:diagonalInertia"], spec.inertia, 0.001) {
                errors.push(spec.path + ": inertia differs from expected value");
            }
        }
        if spec.colliders == () || spec.colliders.len() == 0 {
            errors.push(spec.path + ": no collider coverage was declared");
        } else {
            for collider_spec in spec.colliders {
                let collider = query_prim(doc, collider_spec.path,
                    ["physics:collisionEnabled", "visibility"], (), false);
                let enabled = if query_ok(collider) {
                    collider.attrs["physics:collisionEnabled"]
                } else { () };
                entry.colliders.push(#{ path: collider_spec.path, enabled: enabled });
                if !query_ok(collider) {
                    errors.push(collider_spec.path + ": collider prim is missing");
                } else if collider_spec.enabled != () && enabled != collider_spec.enabled {
                    errors.push(collider_spec.path + ": collisionEnabled is not as authored");
                }
            }
        }
        reports.push(entry);
    }
    #{ ok: errors.len() == 0, reports: reports, errors: errors }
}

/// Build a non-mutating visual explode plan. The returned deltas are consumed
/// by the existing USD preview/transform boundary; this helper never authors a
/// transform and therefore cannot bypass the journal or selection owner.
fn explode_plan(parts, axis, spacing) {
    if !cardinal_axis(axis) || spacing <= 0.0 {
        return #{ ok: false, error: "explode axis must be X/Y/Z and spacing positive" };
    }
    let changes = [];
    for i in 0..parts.len() {
        let distance = spacing * (i + 1);
        let delta = if axis == "X" { [distance, 0.0, 0.0] }
            else if axis == "Y" { [0.0, distance, 0.0] }
            else { [0.0, 0.0, distance] };
        let change = #{ path: parts[i].path, delta: delta, order: i + 1 };
        if parts[i].label != () { change.label = parts[i].label; }
        changes.push(change);
    }
    #{
        ok: true,
        kind: "assembly_explode_plan",
        axis: axis,
        spacing: spacing,
        changes: changes,
    }
}
