// Semantic USD assembly construction tools.
//
// This library is intentionally Rhai-only. It turns common authoring intents
// such as "make a movable cube", "place this part", and "align these edges"
// into the existing typed USD operation plans. The document command layer
// still owns target validation, journalling, composition, and projection.
// Dropping a replacement .rhai in the tool-library path updates this policy
// without a Rust-core change or a second USD writer.

fn plan_error(message) {
    #{ ok: false, error: message, ops: [] }
}

fn cardinal_axis(axis) {
    axis == "X" || axis == "Y" || axis == "Z"
}

fn axis_index(axis) {
    if axis == "X" { return 0; }
    if axis == "Y" { return 1; }
    if axis == "Z" { return 2; }
    -1
}

fn vector3(value, label) {
    if value == () || value.len() != 3 {
        return plan_error(label + " must have three components");
    }
    #{ ok: true, value: value }
}

fn parent_path(path) {
    let parts = path.split("/");
    if parts.len() <= 2 { return "/"; }
    let last = parts.len() - 1;
    let result = "";
    for i in 1..last {
        if i > 1 { result += "/"; }
        result += parts[i];
    }
    if result == "" { "/" } else { "/" + result }
}

/// Return one local transform change set. Rotation is Euler XYZ degrees and
/// scale is unitless, matching the existing Inspector transform contract.
/// The result is a pure typed plan; it does not mutate the document.
fn place_plan(edit_target, path, translation, rotation, scale) {
    if edit_target == () || path == () {
        return plan_error("place_plan requires an edit target and exact prim path");
    }
    if translation == () && rotation == () && scale == () {
        return plan_error("place_plan requires translation, rotation, or scale");
    }
    if translation != () {
        let valid = vector3(translation, "translation");
        if !valid.ok { return valid; }
    }
    if rotation != () {
        let valid = vector3(rotation, "rotation");
        if !valid.ok { return valid; }
    }
    if scale != () {
        let valid = vector3(scale, "scale");
        if !valid.ok { return valid; }
    }

    let ops = [];
    if translation != () {
        ops.push(#{ SetTranslate: #{
            edit_target: edit_target,
            path: path,
            value: translation,
        }});
    }
    if rotation != () {
        ops.push(#{ SetRotate: #{
            edit_target: edit_target,
            path: path,
            value: rotation,
        }});
    }
    if scale != () {
        ops.push(#{ SetScale: #{
            edit_target: edit_target,
            path: path,
            value: scale,
        }});
    }
    #{ ok: true, kind: "placement", path: path, ops: ops }
}

/// Build a standard Cube shape under an existing frame with an explicit scale
/// and collider. This helper never creates a body or a wrapper Xform.
fn cube_shape_plan(edit_target, parent, name, size, scale, collision_enabled) {
    if edit_target == () || parent == () || name == () {
        return plan_error("cube_shape_plan requires an edit target, parent path, and name");
    }
    if size == () || size <= 0.0 {
        return plan_error("cube_shape_plan requires a positive size");
    }
    let shape = parent + "/" + name;
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Cube",
        reference: (),
    }}];
    if scale != () {
        let valid = vector3(scale, "cube scale");
        if !valid.ok { return valid; }
        ops.push(#{ SetScale: #{
            edit_target: edit_target,
            path: shape,
            value: scale,
        }});
    }
    ops.push(#{ SetAttribute: #{
        edit_target: edit_target,
        path: shape,
        name: "size",
        type_name: "double",
        value: size.to_string(),
    }});
    ops.push(#{ SetApiSchemas: #{
        edit_target: edit_target,
        path: shape,
        schemas: ["PhysicsCollisionAPI"],
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: edit_target,
        path: shape,
        name: "physics:collisionEnabled",
        type_name: "bool",
        value: collision_enabled.to_string(),
    }});
    #{ ok: true, kind: "cube_shape", path: shape, ops: ops }
}

/// Build a standard Cube part with an explicit local placement and collider.
/// This is the common visual/collision construction unit used by higher-level
/// assembly policies; it does not silently create a rigid body.
fn cube_plan(edit_target, parent, name, size, translation, rotation, scale,
    collision_enabled) {
    if edit_target == () || parent == () || name == () {
        return plan_error("cube_plan requires an edit target, parent path, and name");
    }
    if size == () || size <= 0.0 {
        return plan_error("cube_plan requires a positive size");
    }
    let path = parent + "/" + name;
    let placement = place_plan(edit_target, path, translation, rotation, scale);
    if !placement.ok && (translation != () || rotation != () || scale != ()) {
        return placement;
    }
    let shape = cube_shape_plan(edit_target, path, "Shape", size, (), collision_enabled);
    if !shape.ok { return shape; }
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Xform",
        reference: (),
    }}];
    if placement.ok {
        for op in placement.ops { ops.push(op); }
    }
    for op in shape.ops { ops.push(op); }
    #{ ok: true, kind: "cube", path: path, shape: shape.path, ops: ops }
}

/// Build one complete movable cube: rigid body, explicit mass/inertia, local
/// placement, and a standard collision shape. Joint ownership remains an
/// explicit separate intent so a missing attachment cannot be hidden.
fn movable_cube_plan(edit_target, parent, name, mass, center_of_mass,
    diagonal_inertia, size, translation, rotation, scale, collision_enabled) {
    let body = assembly_edit::rigid_body_plan(
        edit_target, parent, name, mass, center_of_mass, diagonal_inertia,
    );
    if !body.ok { return body; }
    let shape = cube_shape_plan(edit_target, body.path, "Shape", size,
        scale, collision_enabled);
    if !shape.ok { return shape; }

    let placement = place_plan(edit_target, body.path, translation, rotation, ());
    if !placement.ok { return placement; }
    let ops = [];
    for op in body.ops { ops.push(op); }
    for op in placement.ops { ops.push(op); }
    for op in shape.ops { ops.push(op); }
    #{ ok: true, kind: "movable_cube", path: body.path, shape: shape.path, ops: ops }
}

/// Build a fully explicit revolute hinge through the existing standard-USD
/// planner. The wrapper gives assembly policies one semantic library while
/// keeping the low-level physics contract in the shared assembly editor tool.
fn hinge_plan(edit_target, parent, name, body0, body1, axis,
    local_pos0, local_pos1, local_rot0, local_rot1, lower_limit,
    upper_limit, collision_enabled) {
    return assembly_edit::revolute_joint_plan(
        edit_target, parent, name, body0, body1, axis,
        local_pos0, local_pos1, local_rot0, local_rot1,
        lower_limit, upper_limit, collision_enabled,
    );
}

fn query_ok(record) {
    record != () && record.ok != false
}

/// Read a local translation while rejecting transforms this simple alignment
/// policy cannot interpret. A missing translate opinion is USD identity.
fn local_translation(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        attrs: ["xformOp:translate", "xformOpOrder"],
    });
    if !query_ok(record) {
        return plan_error(path + ": composed prim is unavailable for alignment");
    }
    let order = record.attrs["xformOpOrder"];
    if order != () {
        for op in order {
            if op != "xformOp:translate" {
                return plan_error(path + ": alignment requires translation-only local placement");
            }
        }
    }
    let value = record.attrs["xformOp:translate"];
    if value == () { value = [0.0, 0.0, 0.0]; }
    let valid = vector3(value, "local translation");
    if !valid.ok { return valid; }
    #{ ok: true, value: value }
}

/// Align one exact prim's local centre to another exact prim on one axis.
/// The same authored parent is required, so the calculation never mistakes a
/// local coordinate for a world coordinate. `offset` is measured in metres in
/// that shared parent frame.
fn align_centers_plan(doc, edit_target, moving_path, target_path, axis, offset) {
    if moving_path == () || target_path == () || moving_path == target_path {
        return plan_error("align_centers_plan requires two distinct prim paths");
    }
    if !cardinal_axis(axis) {
        return plan_error("align_centers_plan axis must be X, Y, or Z");
    }
    if offset == () {
        return plan_error("align_centers_plan requires an explicit offset");
    }
    if parent_path(moving_path) != parent_path(target_path) {
        return plan_error("align_centers_plan requires prims with the same authored parent");
    }
    let moving = local_translation(doc, moving_path);
    let target = local_translation(doc, target_path);
    if !moving.ok { return moving; }
    if !target.ok { return target; }
    let value = moving.value;
    let index = axis_index(axis);
    value[index] = target.value[index] + offset;
    let placement = place_plan(edit_target, moving_path, value, (), ());
    placement.kind = "center_alignment";
    placement.reference = target_path;
    placement.axis = axis;
    placement.offset = offset;
    placement
}

/// Align explicit axis-aligned Cube edges in a shared parent frame. Edge
/// signs are -1 or +1 relative to each cube centre. Rotated parent parts are
/// rejected instead of producing a plausible but wrong placement.
fn align_cube_edges_plan(doc, edit_target, moving_path, moving_shape,
    target_path, target_shape, axis, moving_edge, target_edge, gap) {
    if moving_path == () || target_path == () || moving_shape == () || target_shape == () {
        return plan_error("align_cube_edges_plan requires exact prim and shape paths");
    }
    if moving_path == target_path || !cardinal_axis(axis) {
        return plan_error("align_cube_edges_plan requires distinct paths and a cardinal axis");
    }
    if moving_edge != -1 && moving_edge != 1 {
        return plan_error("moving_edge must be -1 or +1");
    }
    if target_edge != -1 && target_edge != 1 {
        return plan_error("target_edge must be -1 or +1");
    }
    if gap == () || gap < 0.0 {
        return plan_error("align_cube_edges_plan requires a non-negative gap");
    }
    if parent_path(moving_path) != parent_path(target_path) {
        return plan_error("align_cube_edges_plan requires prims with the same authored parent");
    }
    let moving = local_translation(doc, moving_path);
    let target = local_translation(doc, target_path);
    if !moving.ok { return moving; }
    if !target.ok { return target; }
    let moving_extents = assembly_audit::axis_aligned_cube_half_extents(doc, moving_shape);
    let target_extents = assembly_audit::axis_aligned_cube_half_extents(doc, target_shape);
    if moving_extents == () || target_extents == () {
        return plan_error("align_cube_edges_plan requires axis-aligned Cube shape extents");
    }

    let index = axis_index(axis);
    let value = moving.value;
    value[index] = target.value[index]
        + target_edge * target_extents[index]
        - moving_edge * moving_extents[index]
        + target_edge * gap;
    let placement = place_plan(edit_target, moving_path, value, (), ());
    placement.kind = "cube_edge_alignment";
    placement.reference = target_path;
    placement.axis = axis;
    placement.moving_edge = moving_edge;
    placement.target_edge = target_edge;
    placement.gap = gap;
    placement
}
