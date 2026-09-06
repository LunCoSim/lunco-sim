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

fn variant_selection_values(path, variants) {
    let selections = [];
    if variants == () { return #{ ok: true, value: selections }; }
    let seen = [];
    for selection in variants {
        if selection == () || selection.variant_set == ()
            || selection.variant == () || selection.variant_set == ""
            || selection.variant == "" || seen.contains(selection.variant_set) {
            return plan_error(path + ": variants require unique variant_set and variant fields");
        }
        seen.push(selection.variant_set);
        selections.push(selection);
    }
    #{ ok: true, value: selections }
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

/// Build one referenced component instance without writing USDA source.
///
/// The reference is the component's identity and composition boundary; local
/// schemas, attributes, and transform opinions are explicit instance
/// decisions. Variant selections have a separate second-phase plan because
/// they recompose the subtree and must not race first-use reference loading.
/// The parent and target are inspected before a plan is returned, so a recipe
/// cannot silently create a duplicate or write below a missing frame. The
/// typed document owner still validates the reference, composition, and
/// operation ordering when the plan is reviewed.
fn referenced_instance_plan(doc, edit_target, parent, name, type_name, asset,
    translation, rotation, scale, api_schemas, attributes) {
    referenced_instance_targeted_plan(
        doc, edit_target, parent, name, type_name, asset, (),
        translation, rotation, scale, api_schemas, attributes,
    )
}

/// Build a referenced component instance with an optional explicit prim target.
/// `reference_prim_path == ()` uses the referenced layer's defaultPrim. A
/// targeted reference is useful when a composition asset owns variants on a
/// named prim and the dynamic authoring path must preserve that identity.
fn referenced_instance_targeted_plan(doc, edit_target, parent, name, type_name, asset,
    reference_prim_path, translation, rotation, scale, api_schemas, attributes) {
    if doc == () || edit_target == () || parent == () || name == ()
        || type_name == () || asset == () {
        return plan_error("referenced_instance_plan requires document, edit target, parent, name, type, and asset");
    }
    if !parent.starts_with("/") || parent.ends_with("/") && parent != "/" {
        return plan_error("referenced_instance_plan parent must be an absolute USD path");
    }
    if name == "" || name.contains("/") {
        return plan_error("referenced_instance_plan name must be one USD identifier");
    }
    if type_name == "" {
        return plan_error("referenced_instance_plan requires an explicit USD type");
    }
    if !asset.starts_with("lunco://") || asset.len() <= "lunco://".len()
        || asset.contains(" ") || asset.contains("@") {
        return plan_error("referenced_instance_plan asset must be a non-empty lunco:// URI without USDA delimiters");
    }
    if reference_prim_path != () && (!reference_prim_path.starts_with("/")
        || reference_prim_path.ends_with("/")) {
        return plan_error("referenced_instance_plan reference prim path must be an absolute prim path");
    }

    let parent_record = query("QueryUsdPrim", #{
        doc: doc,
        path: parent,
        children: false,
    });
    if !query_ok(parent_record) {
        return plan_error(parent + ": referenced instance parent is unavailable");
    }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let existing = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        children: false,
    });
    if query_ok(existing) {
        return plan_error(path + ": referenced instance identity is already occupied");
    }

    let schemas = [];
    if api_schemas != () {
        let seen = [];
        for schema in api_schemas {
            if schema == () || schema == "" || seen.contains(schema) {
                return plan_error(path + ": API schema list contains an empty or duplicate entry");
            }
            seen.push(schema);
            schemas.push(schema);
        }
    }

    let attrs = [];
    if attributes != () {
        let seen = [];
        for attribute in attributes {
            if attribute == () || attribute.name == () || attribute.type_name == ()
                || attribute.value == () || attribute.name == ""
                || attribute.type_name == "" || seen.contains(attribute.name) {
                return plan_error(path + ": attributes require unique name, type_name, and value fields");
            }
            seen.push(attribute.name);
            attrs.push(attribute);
        }
    }

    let authored_reference_prim_path = if reference_prim_path == () {
        ""
    } else {
        reference_prim_path
    };
    let add_prim = #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: type_name,
        reference: asset,
        reference_prim_path: authored_reference_prim_path,
    };
    let ops = [#{ AddPrim: add_prim }];
    if schemas.len() > 0 {
        ops.push(#{ SetApiSchemas: #{
            edit_target: edit_target,
            path: path,
            schemas: schemas,
        }});
    }
    for attribute in attrs {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target,
            path: path,
            name: attribute.name,
            type_name: attribute.type_name,
            value: attribute.value.to_string(),
        }});
    }
    if translation != () || rotation != () || scale != () {
        let placement = place_plan(edit_target, path, translation, rotation, scale);
        if !placement.ok { return placement; }
        for op in placement.ops { ops.push(op); }
    }
    #{
        ok: true,
        kind: "referenced_instance",
        path: path,
        asset: asset,
        ops: ops,
    }
}

/// Build a second-phase variant-selection plan for an already materialized
/// referenced instance. Variant changes recompose a subtree, so first-use
/// reference materialization must complete before this plan is proposed. The
/// plan is still pure and dynamically reloadable; the document owner remains
/// responsible for the coarse rebuild and journal boundary.
fn select_variants_plan(doc, edit_target, path, variants) {
    if doc == () || edit_target == () || path == () || variants == () {
        return plan_error("select_variants_plan requires document, edit target, path, and variants");
    }
    if !path.starts_with("/") || path.ends_with("/") {
        return plan_error("select_variants_plan path must be an absolute USD prim path");
    }
    let record = query("QueryUsdPrim", #{ doc: doc, path: path, children: false });
    if !query_ok(record) {
        return plan_error(path + ": variant-selection target is unavailable");
    }
    let valid = variant_selection_values(path, variants);
    if !valid.ok { return valid; }
    if valid.value.len() == 0 {
        return plan_error(path + ": select_variants_plan requires at least one selection");
    }
    let ops = [];
    for selection in valid.value {
        ops.push(#{ SetVariantSelection: #{
            edit_target: edit_target,
            path: path,
            variant_set: selection.variant_set,
            variant: selection.variant,
        }});
    }
    #{
        ok: true,
        kind: "variant_selection",
        path: path,
        variants: valid.value,
        ops: ops,
    }
}

/// Build an empty authored Xform frame with an optional local placement.
///
/// Frames are the namespace and transform owner for authored parts; geometry,
/// collision, body, and joint contracts remain separate explicit plans. This
/// small unit lets higher-level Rhai recipes create a clean hierarchy without
/// spelling a raw USD prim operation at every call site.
fn frame_plan(edit_target, parent, name, translation, rotation, scale) {
    if edit_target == () || parent == () || name == () || name == ""
        || name.contains("/") {
        return plan_error("frame_plan requires an edit target, parent, and one Xform name");
    }
    if !parent.starts_with("/") || parent.ends_with("/") && parent != "/" {
        return plan_error("frame_plan parent must be an absolute USD path");
    }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let placement = place_plan(edit_target, path, translation, rotation, scale);
    if !placement.ok && (translation != () || rotation != () || scale != ()) {
        return placement;
    }
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Xform",
        reference: (),
        reference_prim_path: "",
    }}];
    if placement.ok {
        for op in placement.ops { ops.push(op); }
    }
    #{ ok: true, kind: "frame", path: path, ops: ops }
}

/// Instantiate a complete four-wheel rover assembly from one maintained USD
/// vehicle asset. The asset owns its chassis, wheel, suspension, drivetrain,
/// and controller composition; this recipe owns only the explicit instance
/// identity, placement, and expected wheel paths. Variant choices are applied
/// afterward through `select_variants_plan` once the reference closure is
/// materialized. The paths are checked before proposal for uniqueness and
/// containment, then the production scenario inspects their composed
/// Cylinder/index/dimension contract after the reference is committed.
fn flip_rover_instance_plan(doc, edit_target, parent, name, asset,
    translation, rotation, wheel_paths) {
    if wheel_paths == () || wheel_paths.len() != 4 {
        return plan_error("flip_rover_instance_plan requires exactly four explicit wheel paths");
    }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let seen = [];
    for wheel_path in wheel_paths {
        if wheel_path == () || !wheel_path.starts_with(path + "/")
            || seen.contains(wheel_path) {
            return plan_error("flip_rover_instance_plan wheel paths must be unique descendants of the instance");
        }
        seen.push(wheel_path);
    }
    let plan = referenced_instance_plan(
        doc, edit_target, parent, name, "Xform", asset,
        translation, rotation, (), (), (),
    );
    if !plan.ok { return plan; }
    plan.kind = "flip_rover_instance";
    plan.wheel_paths = seen;
    plan
}

/// Build a standard Cube shape under an existing frame with an explicit scale
/// and collider. This helper never creates a body or a wrapper Xform.
fn cube_shape_plan(edit_target, parent, name, size, scale, collision_enabled) {
    if edit_target == () || parent == () || name == () || name == ""
        || name.contains("/") {
        return plan_error("cube_shape_plan requires an edit target, parent path, and name");
    }
    if !parent.starts_with("/") || parent.ends_with("/") && parent != "/" {
        return plan_error("cube_shape_plan parent must be an absolute USD path");
    }
    if size == () || size <= 0.0 {
        return plan_error("cube_shape_plan requires a positive size");
    }
    let shape = if parent == "/" { "/" + name } else { parent + "/" + name };
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Cube",
        reference: (),
        reference_prim_path: "",
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

/// Build a standard Cylinder shape with explicit dimensions, placement, and
/// collision ownership. Like `cube_shape_plan`, this never creates a body or
/// a wrapper frame; a caller must choose whether the shape is physical or
/// explicitly visual-only.
fn cylinder_shape_plan(edit_target, parent, name, radius, height,
    translation, rotation, scale, collision_enabled) {
    if edit_target == () || parent == () || name == () || name == ""
        || name.contains("/") {
        return plan_error("cylinder_shape_plan requires an edit target, parent, and one Cylinder name");
    }
    if !parent.starts_with("/") || parent.ends_with("/") && parent != "/" {
        return plan_error("cylinder_shape_plan parent must be an absolute USD path");
    }
    if radius == () || radius <= 0.0 || height == () || height <= 0.0 {
        return plan_error("cylinder_shape_plan requires positive radius and height");
    }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let placement = place_plan(edit_target, path, translation, rotation, scale);
    if !placement.ok && (translation != () || rotation != () || scale != ()) {
        return placement;
    }
    if scale != () {
        for component in scale {
            if component <= 0.0 {
                return plan_error(path + ": cylinder scale components must be positive");
            }
        }
    }
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Cylinder",
        reference: (),
        reference_prim_path: "",
    }}];
    if placement.ok {
        for op in placement.ops { ops.push(op); }
    }
    ops.push(#{ SetAttribute: #{
        edit_target: edit_target,
        path: path,
        name: "radius",
        type_name: "double",
        value: radius.to_string(),
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: edit_target,
        path: path,
        name: "height",
        type_name: "double",
        value: height.to_string(),
    }});
    ops.push(#{ SetApiSchemas: #{
        edit_target: edit_target,
        path: path,
        schemas: ["PhysicsCollisionAPI"],
    }});
    ops.push(#{ SetAttribute: #{
        edit_target: edit_target,
        path: path,
        name: "physics:collisionEnabled",
        type_name: "bool",
        value: collision_enabled.to_string(),
    }});
    #{ ok: true, kind: "cylinder_shape", path: path, ops: ops }
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
        reference_prim_path: "",
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

/// Promote one existing composed Xform into an explicitly massed movable
/// body. This is the retrofit counterpart to `movable_cube_plan`: referenced
/// parts already own their visual/collision children, so the builder
/// must add the body opinion without replacing or duplicating that geometry.
/// Every collider path is explicit and must already exist below the body;
/// body/joint validators still own the final schema and relationship checks.
/// The first operation defines a local Xform over the composed target. It does
/// not copy the referenced children; it creates the authored prim spec required
/// for persistent schema and mass opinions to land in @root@.
fn existing_rigid_body_plan(doc, edit_target, path, mass, center_of_mass,
    diagonal_inertia, collider_paths) {
    if doc == () || edit_target == () || path == () {
        return plan_error("existing_rigid_body_plan requires a document, edit target, and exact body path");
    }
    if mass == () || mass <= 0.0 {
        return plan_error("existing_rigid_body_plan requires positive mass");
    }
    let com = assembly_edit::tuple3_literal(center_of_mass);
    let inertia = assembly_edit::tuple3_literal(diagonal_inertia);
    if com == () || inertia == () {
        return plan_error("existing_rigid_body_plan requires three-component centre of mass and inertia");
    }
    if collider_paths == () || collider_paths.len() == 0 {
        return plan_error("existing_rigid_body_plan requires explicit collider paths");
    }

    let body = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        attrs: ["physics:mass", "physics:centerOfMass", "physics:diagonalInertia"],
        children: false,
    });
    if !query_ok(body) {
        return plan_error(path + ": existing body prim is unavailable");
    }
    if body.type_name != "Xform" {
        return plan_error(path + ": existing movable body must be an Xform");
    }
    if body.attrs["physics:mass"] != ()
        && body.attrs["physics:centerOfMass"] != ()
        && body.attrs["physics:diagonalInertia"] != () {
        return plan_error(path + ": existing prim already has a complete body contract; use an attribute/transform edit instead of promoting it again");
    }

    let checked = [];
    for collider_path in collider_paths {
        if collider_path == () || !collider_path.starts_with(path + "/") {
            return plan_error(collider_path + ": collider must be an exact descendant of the body");
        }
        if checked.contains(collider_path) {
            return plan_error(collider_path + ": collider path is listed more than once");
        }
        let collider = query("QueryUsdPrim", #{
            doc: doc,
            path: collider_path,
            attrs: ["physics:collisionEnabled"],
        });
        if !query_ok(collider) {
            return plan_error(collider_path + ": explicit collider prim is unavailable");
        }
        if collider.type_name == "Xform" || collider.type_name == "Scope" {
            return plan_error(collider_path + ": collider path must name a geometric prim");
        }
        checked.push(collider_path);
    }

    #{
        ok: true,
        kind: "existing_rigid_body",
        path: path,
        colliders: checked,
        ops: [
            #{ AddPrim: #{
                edit_target: edit_target,
                parent_path: parent_path(path),
                name: path.split("/")[path.split("/").len() - 1],
                type_name: "Xform",
                reference: (),
                reference_prim_path: "",
            }},
            #{ SetApiSchemas: #{
                edit_target: edit_target,
                path: path,
                schemas: ["PhysicsRigidBodyAPI", "PhysicsMassAPI"],
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:mass",
                type_name: "float",
                value: mass.to_string(),
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:centerOfMass",
                type_name: "point3f",
                value: com,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: path,
                name: "physics:diagonalInertia",
                type_name: "float3",
                value: inertia,
            }},
        ],
    }
}

/// Build one existing referenced part and its required revolute attachment as
/// a single reviewed plan. The body path, joint parent, both bodies, frames,
/// axis, limits, and collider paths remain explicit; the helper only prevents
/// callers from accidentally promoting a part and forgetting its joint.
fn existing_hinged_body_plan(doc, edit_target, body_path, mass,
    center_of_mass, diagonal_inertia, collider_paths, joint_parent,
    joint_name, body0, axis, local_pos0, local_pos1, local_rot0, local_rot1,
    lower_limit, upper_limit, collision_enabled) {
    if joint_parent == () || joint_name == () || body0 == () {
        return plan_error("existing_hinged_body_plan requires an explicit joint parent, name, and body0");
    }
    if joint_parent != parent_path(body_path) {
        return plan_error("existing_hinged_body_plan requires the joint to be a sibling of the body");
    }
    if body0 == body_path {
        return plan_error("existing_hinged_body_plan requires distinct body0 and body1 paths");
    }

    let body = existing_rigid_body_plan(doc, edit_target, body_path, mass,
        center_of_mass, diagonal_inertia, collider_paths);
    if !body.ok { return body; }
    let joint = hinge_plan(edit_target, joint_parent, joint_name, body0,
        body_path, axis, local_pos0, local_pos1, local_rot0, local_rot1,
        lower_limit, upper_limit, collision_enabled);
    if !joint.ok { return joint; }

    let ops = [];
    for op in body.ops { ops.push(op); }
    for op in joint.ops { ops.push(op); }
    #{
        ok: true,
        kind: "existing_hinged_body",
        path: body_path,
        joint: joint.path,
        colliders: body.colliders,
        ops: ops,
    }
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

/// Resolve exactly one empty, authored socket for a reusable component.
///
/// The host's `lunco:mount:sockets` relationship is the source of truth. This
/// helper never searches by a leaf name or invents a socket from a grouping
/// convention; zero or multiple compatible sockets fail before the attach
/// command is submitted. The returned `joint` value is the reflected typed
/// AttachJoint expected by `assembly_edit::attach_component`.
fn find_compatible_socket(doc, host_path, socket_kind, joint_kind, asset,
    require_empty) {
    if doc == () || host_path == () || socket_kind == () || joint_kind == ()
        || asset == () {
        return plan_error("find_compatible_socket requires document, host, socket kind, joint kind, and asset");
    }

    let host = query("QueryUsdPrim", #{
        doc: doc,
        path: host_path,
        rels: ["lunco:mount:sockets"],
    });
    if !query_ok(host) || host.relationships == ()
        || host.relationships["lunco:mount:sockets"] == () {
        return plan_error(host_path + ": host has no authored mount sockets");
    }

    let candidates = [];
    for socket_path in host.relationships["lunco:mount:sockets"] {
        let socket = query("QueryUsdPrim", #{
            doc: doc,
            path: socket_path,
            attrs: [
                "lunco:mount:socket",
                "lunco:mount:joint",
                "lunco:mount:axis",
                "lunco:mount:asset",
            ],
            rels: ["lunco:mount:part"],
        });
        if !query_ok(socket) || socket.attrs == () { continue; }
        if socket.attrs["lunco:mount:socket"] != socket_kind
            || socket.attrs["lunco:mount:joint"] != joint_kind
            || socket.attrs["lunco:mount:asset"] != asset {
            continue;
        }

        let occupied = socket.relationships["lunco:mount:part"];
        if require_empty && occupied != () && occupied.len() != 0 { continue; }

        let joint = socket.attrs["lunco:mount:joint"];
        let joint_value = if joint == "fixed" {
            "Fixed"
        } else if joint == "revolute" || joint == "prismatic" {
            let axis = socket.attrs["lunco:mount:axis"];
            if !cardinal_axis(axis) {
                return plan_error(socket_path + ": articulated socket requires X, Y, or Z axis");
            }
            if joint == "revolute" {
                #{ Revolute: #{ axis: axis } }
            } else {
                #{ Prismatic: #{ axis: axis } }
            }
        } else {
            return plan_error(socket_path + ": unsupported authored mount joint kind");
        };
        candidates.push(#{ path: socket_path, joint: joint_value });
    }

    if candidates.len() == 0 {
        return plan_error(host_path + ": no compatible authored mount socket");
    }
    if candidates.len() != 1 {
        return plan_error(host_path + ": compatible mount socket is ambiguous");
    }
    #{
        ok: true,
        socket_path: candidates[0].path,
        joint: candidates[0].joint,
    }
}

/// Attach a reusable component by its authored host/socket contract.
///
/// The command owner performs plug-frame resolution, reference lowering,
/// joint creation, occupancy, and one-change-set journalling. This wrapper
/// supplies only semantic intent: select the unique socket, derive the typed
/// joint from its metadata, and ask the existing owner to snap the component
/// into place. Neutral placement/rotation values are required by the public
/// AttachSpec but are not used to guess the socket frame.
fn mount_component(doc, edit_target, host_path, socket_kind, joint_kind,
    asset, name, joint_name) {
    if edit_target == () || name == () || joint_name == () {
        return plan_error("mount_component requires edit target, component name, and joint name");
    }
    let selected = find_compatible_socket(doc, host_path, socket_kind,
        joint_kind, asset, true);
    if !selected.ok { return selected; }

    let result = assembly_edit::attach_component(doc, #{
        edit_target: edit_target,
        socket_path: selected.socket_path,
        host_path: host_path,
        name: name,
        joint_name: joint_name,
        asset: asset,
        placement: [0.0, 0.0, 0.0],
        rotate_deg: [0.0, 0.0, 0.0],
        joint: selected.joint,
    });
    if result.ok {
        result.kind = "socket_mount";
        result.socket_path = selected.socket_path;
    }
    result
}

// ── Existing-mount frame realignment ───────────────────────────────────────

fn finite_vector3(value, label) {
    if value == () || value.len() != 3 {
        return plan_error(label + " must have three components");
    }
    for component in value {
        if !component.is_finite() {
            return plan_error(label + " must contain finite components");
        }
    }
    #{ ok: true, value: value }
}

// Quaternion multiplication for the [x,y,z,w] representation used by the
// public qrot() helper. The product has the same convention as Bevy/GLAM:
// q_mul(a,b) applies b, then a.
fn mount_quat_mul(a, b) {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn mount_quat_inverse(q) {
    let length_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    [
        -q[0] / length_sq,
        -q[1] / length_sq,
        -q[2] / length_sq,
        q[3] / length_sq,
    ]
}

// USD rotateXYZ is the fixed-axis/extrinsic sequence used by the existing
// typed SetRotate contract. GLAM expresses it as Rz * Ry * Rx.
fn mount_quat_from_xyz_deg(rotation) {
    let half_x = rotation[0] * 0.008726646259971648;
    let half_y = rotation[1] * 0.008726646259971648;
    let half_z = rotation[2] * 0.008726646259971648;
    let qx = [half_x.sin(), 0.0, 0.0, half_x.cos()];
    let qy = [0.0, half_y.sin(), 0.0, half_y.cos()];
    let qz = [0.0, 0.0, half_z.sin(), half_z.cos()];
    mount_quat_mul(qz, mount_quat_mul(qy, qx))
}

fn mount_atan2(y, x) {
    let half_pi = 1.5707963267948966;
    let pi = 3.141592653589793;
    if x > 0.0 { return (y / x).atan(); }
    if x < 0.0 && y >= 0.0 { return (y / x).atan() + pi; }
    if x < 0.0 && y < 0.0 { return (y / x).atan() - pi; }
    if y > 0.0 { return half_pi; }
    if y < 0.0 { return -half_pi; }
    0.0
}

// Return the canonical XYZ degrees accepted by SetRotate. This is the same
// decomposition as GLAM's EulerRot::XYZEx for Rz * Ry * Rx. The gimbal-lock
// branch chooses zero Z, matching the shared typed transform reader.
fn mount_xyz_deg_from_quat(q) {
    let x = q[0];
    let y = q[1];
    let z = q[2];
    let w = q[3];
    let m00 = 1.0 - 2.0 * (y * y + z * z);
    let m10 = 2.0 * (x * y + w * z);
    let m20 = 2.0 * (x * z - w * y);
    let m21 = 2.0 * (y * z + w * x);
    let m22 = 1.0 - 2.0 * (x * x + y * y);
    let cy = (m00 * m00 + m10 * m10).sqrt();
    let radians_to_degrees = 57.29577951308232;
    if cy > 1.0e-8 {
        [
            mount_atan2(m21, m22) * radians_to_degrees,
            (-m20).asin() * radians_to_degrees,
            mount_atan2(m10, m00) * radians_to_degrees,
        ]
    } else {
        [
            mount_atan2(-2.0 * (x * y - w * z), w * w + x * x - y * y - z * z)
                * radians_to_degrees,
            (-m20).asin() * radians_to_degrees,
            0.0,
        ]
    }
}

fn mount_transform_identity() {
    #{ translation: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0] }
}

// Compose a * b in the canonical stage frame. Scale is deliberately absent:
// mount frames are rigid contracts, so a non-unit scale is rejected before a
// proposal rather than silently baking scale into a rigid part placement.
fn mount_transform_compose(a, b) {
    #{
        translation: vadd(a.translation, qrot(a.rotation, b.translation)),
        rotation: mount_quat_mul(a.rotation, b.rotation),
    }
}

fn mount_transform_inverse(value) {
    let inverse_rotation = mount_quat_inverse(value.rotation);
    #{
        translation: vscale(qrot(inverse_rotation, value.translation), -1.0),
        rotation: inverse_rotation,
    }
}

fn mount_local_transform(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc: doc,
        path: path,
        attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale", "xformOpOrder"],
    });
    if !query_ok(record) {
        return plan_error(path + ": composed frame prim is unavailable");
    }

    let translate = record.attrs["xformOp:translate"];
    let rotate = record.attrs["xformOp:rotateXYZ"];
    let scale = record.attrs["xformOp:scale"];
    let order = record.attrs["xformOpOrder"];
    if order == () {
        if translate != () || rotate != () || scale != () {
            return plan_error(path + ": frame transform data requires an explicit xformOpOrder");
        }
        return #{ ok: true, value: mount_transform_identity() };
    }

    let result = mount_transform_identity();
    let seen = [];
    for op in order {
        if op == "!resetXformStack!" || op.starts_with("!invert!") {
            return plan_error(path + ": frame transforms cannot use reset or inverse xform ops");
        }
        if seen.contains(op) {
            return plan_error(path + ": frame xformOpOrder repeats " + op);
        }
        seen.push(op);
        let operation = mount_transform_identity();
        if op == "xformOp:translate" {
            let valid = finite_vector3(translate, path + " translation");
            if !valid.ok { return valid; }
            operation.translation = valid.value;
        } else if op == "xformOp:rotateXYZ" {
            let valid = finite_vector3(rotate, path + " rotateXYZ");
            if !valid.ok { return valid; }
            operation.rotation = mount_quat_from_xyz_deg(valid.value);
        } else if op == "xformOp:scale" {
            let valid = finite_vector3(scale, path + " scale");
            if !valid.ok { return valid; }
            for component in valid.value {
                if (component - 1.0).abs() > 1.0e-9 {
                    return plan_error(path + ": mount frames require unit scale");
                }
            }
        } else {
            return plan_error(path + ": unsupported rigid frame operation " + op);
        }
        // USD's ordered stack is multiplied left-to-right. Thus the first
        // listed operation is the left matrix factor, as in the typed reader.
        result = mount_transform_compose(result, operation);
    }
    #{ ok: true, value: result }
}

// Compose a frame below a body root while intentionally excluding the body
// root's own placement. This mirrors the existing mount reader, but remains a
// dynamically replaceable authoring policy: only the resulting generic typed
// ops cross into Rust.
fn mount_frame_in_body(doc, body_root, frame_path) {
    if frame_path != body_root && !frame_path.starts_with(body_root + "/") {
        return plan_error(frame_path + ": frame is outside body root " + body_root);
    }
    if frame_path == body_root {
        return #{ ok: true, value: mount_transform_identity() };
    }

    let current = frame_path;
    let accumulated = mount_local_transform(doc, current);
    if !accumulated.ok { return accumulated; }
    let value = accumulated.value;
    while parent_path(current) != body_root {
        current = parent_path(current);
        let local = mount_local_transform(doc, current);
        if !local.ok { return local; }
        value = mount_transform_compose(local.value, value);
    }
    #{ ok: true, value: value }
}

fn mount_relationship_targets(record, name) {
    if !query_ok(record) || record.relationships == () || record.relationships[name] == () {
        return [];
    }
    record.relationships[name]
}

fn mount_count_target(targets, expected) {
    let count = 0;
    for target in targets {
        if target == expected { count += 1; }
    }
    count
}

/// Build one atomic plan to realign an already attached part to its explicit
/// socket and plug frames. This is the dynamic counterpart of the Editor's
/// mount snap: moving either authored frame is enough to recalculate the part
/// and the exact recorded joint anchor. It never creates or removes topology.
///
/// The reader deliberately supports only canonical, rigid `translate` and
/// `rotateXYZ` stacks with unit scale. Unsupported matrices, inverse stacks,
/// non-unit scales, malformed values, missing relationships, and mismatched
/// body identities fail before proposal/commit. That narrow contract keeps the
/// tool safe to hot-reload while the generic USD owner remains authoritative
/// for target validation, journalling, and projection.
fn mount_frame_realignment_plan(doc, edit_target, host_path, socket_path,
    part_path, joint_path) {
    if doc == () || edit_target == () || host_path == () || socket_path == ()
        || part_path == () || joint_path == () {
        return plan_error("mount_frame_realignment_plan requires document, edit target, host, socket, part, and joint paths");
    }
    for path in [host_path, socket_path, part_path, joint_path] {
        if !path.starts_with("/") || path.ends_with("/") {
            return plan_error("mount_frame_realignment_plan paths must be absolute USD paths");
        }
    }
    if part_path == host_path || !part_path.starts_with(host_path + "/") {
        return plan_error("mounted part must be a strict descendant of the host body");
    }
    if socket_path == host_path || !socket_path.starts_with(host_path + "/") {
        return plan_error("socket must be a strict descendant of the host body");
    }

    let host = query("QueryUsdPrim", #{
        doc: doc,
        path: host_path,
        rels: ["lunco:mount:sockets"],
    });
    if !query_ok(host) {
        return plan_error(host_path + ": mount host is unavailable");
    }
    let advertised = mount_relationship_targets(host, "lunco:mount:sockets");
    if mount_count_target(advertised, socket_path) != 1 {
        return plan_error(socket_path + ": socket must be advertised exactly once by its host");
    }

    let socket = query("QueryUsdPrim", #{
        doc: doc,
        path: socket_path,
        attrs: ["lunco:mount:socket", "lunco:mount:joint", "lunco:mount:axis"],
        rels: ["lunco:mount:part"],
    });
    if !query_ok(socket) {
        return plan_error(socket_path + ": authored socket is unavailable");
    }
    let socket_kind = socket.attrs["lunco:mount:socket"];
    let joint_kind = socket.attrs["lunco:mount:joint"];
    if socket_kind == () || socket_kind == "" || joint_kind == () || joint_kind == "" {
        return plan_error(socket_path + ": socket requires explicit mount kind and joint kind");
    }
    if joint_kind != "fixed" && joint_kind != "revolute" && joint_kind != "prismatic" {
        return plan_error(socket_path + ": unsupported authored mount joint kind");
    }
    if (joint_kind == "revolute" || joint_kind == "prismatic")
        && !cardinal_axis(socket.attrs["lunco:mount:axis"]) {
        return plan_error(socket_path + ": articulated socket requires X, Y, or Z axis");
    }
    let occupied = mount_relationship_targets(socket, "lunco:mount:part");
    if occupied.len() != 1 || occupied[0] != part_path {
        return plan_error(socket_path + ": socket occupancy must name the exact mounted part");
    }

    let part = query("QueryUsdPrim", #{
        doc: doc,
        path: part_path,
        attrs: ["lunco:mount:plug"],
        rels: ["lunco:mount:frame", "lunco:mount:attachmentJoint"],
    });
    if !query_ok(part) {
        return plan_error(part_path + ": mounted part is unavailable");
    }
    if part.attrs["lunco:mount:plug"] != socket_kind {
        return plan_error(part_path + ": plug kind does not match the authored socket kind");
    }
    let plug_frames = mount_relationship_targets(part, "lunco:mount:frame");
    if plug_frames.len() != 1 {
        return plan_error(part_path + ": mount frame relationship must have exactly one target");
    }
    let plug_path = plug_frames[0];
    if plug_path == part_path || !plug_path.starts_with(part_path + "/") {
        return plan_error(plug_path + ": plug frame must be a descendant of its part");
    }
    let attachment_joints = mount_relationship_targets(part, "lunco:mount:attachmentJoint");
    if attachment_joints.len() != 1 || attachment_joints[0] != joint_path {
        return plan_error(part_path + ": attachment joint relationship must name the exact supplied joint");
    }

    let joint = query("QueryUsdPrim", #{
        doc: doc,
        path: joint_path,
        rels: ["physics:body0", "physics:body1"],
    });
    if !query_ok(joint) {
        return plan_error(joint_path + ": attachment joint is unavailable");
    }
    let body0 = mount_relationship_targets(joint, "physics:body0");
    let body1 = mount_relationship_targets(joint, "physics:body1");
    if body0.len() != 1 || body0[0] != host_path
        || body1.len() != 1 || body1[0] != part_path {
        return plan_error(joint_path + ": joint body relationships do not match host and part");
    }

    let socket_frame = mount_frame_in_body(doc, host_path, socket_path);
    if !socket_frame.ok { return socket_frame; }
    let plug_frame = mount_frame_in_body(doc, part_path, plug_path);
    if !plug_frame.ok { return plug_frame; }
    let placement = mount_transform_compose(
        socket_frame.value, mount_transform_inverse(plug_frame.value),
    );
    let placement_rotation = mount_xyz_deg_from_quat(placement.rotation);
    let translation = finite_vector3(placement.translation, "resolved mount translation");
    if !translation.ok { return translation; }
    let rotation = finite_vector3(placement_rotation, "resolved mount rotation");
    if !rotation.ok { return rotation; }
    let local_pos0 = assembly_edit::tuple3_literal(placement.translation);
    let local_pos1 = assembly_edit::tuple3_literal([0.0, 0.0, 0.0]);
    if local_pos0 == () || local_pos1 == () {
        return plan_error("resolved mount placement could not be serialized");
    }

    #{
        ok: true,
        kind: "mount_frame_realignment",
        host_path: host_path,
        socket_path: socket_path,
        part_path: part_path,
        plug_path: plug_path,
        joint_path: joint_path,
        placement: placement.translation,
        rotation: placement_rotation,
        ops: [
            #{ SetTranslate: #{
                edit_target: edit_target,
                path: part_path,
                value: placement.translation,
            }},
            #{ SetRotate: #{
                edit_target: edit_target,
                path: part_path,
                value: placement_rotation,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: joint_path,
                name: "physics:localPos0",
                type_name: "point3f",
                value: local_pos0,
            }},
            #{ SetAttribute: #{
                edit_target: edit_target,
                path: joint_path,
                name: "physics:localPos1",
                type_name: "point3f",
                value: local_pos1,
            }},
        ],
    }
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

/// Place one translation-only Cube frame while enforcing an explicit minimum
/// AABB clearance from every listed blocker. All frames and shapes are exact
/// composed paths. The common parent requirement makes the candidate and the
/// queried blocker positions comparable without inventing world-transform or
/// shape semantics; callers with rotated geometry must supply a different
/// authored contract rather than receiving a plausible placement.
fn place_with_clearance_plan(doc, edit_target, moving_path, moving_shape,
    translation, blockers, minimum_gap) {
    if doc == () || edit_target == () || moving_path == () || moving_shape == ()
        || blockers == () || blockers.len() == 0 {
        return plan_error("place_with_clearance_plan requires a document, moving Cube, and explicit blockers");
    }
    if minimum_gap == () || !minimum_gap.is_finite() || minimum_gap < 0.0 {
        return plan_error("place_with_clearance_plan requires a finite non-negative minimum gap");
    }
    let valid_translation = vector3(translation, "candidate translation");
    if !valid_translation.ok { return valid_translation; }
    if !moving_shape.starts_with(moving_path + "/") {
        return plan_error("moving shape must be an exact descendant of the moving prim");
    }

    let parent = parent_path(moving_path);
    if parent != "/" {
        let parent_frame = local_translation(doc, parent);
        if !parent_frame.ok {
            return plan_error(parent + ": clearance requires a translation-only authored parent frame");
        }
    }
    let moving = local_translation(doc, moving_path);
    if !moving.ok { return moving; }
    let moving_extents = assembly_audit::axis_aligned_cube_half_extents(doc, moving_shape);
    if moving_extents == () {
        return plan_error(moving_shape + ": moving shape must be a composed axis-aligned Cube");
    }

    let seen_paths = [];
    let seen_shapes = [];
    for blocker in blockers {
        if blocker == () || blocker.path == () || blocker.shape == () {
            return plan_error("each clearance blocker requires exact path and shape fields");
        }
        if blocker.path == moving_path || seen_paths.contains(blocker.path)
            || seen_shapes.contains(blocker.shape) {
            return plan_error("clearance blockers must be unique and distinct from the moving prim");
        }
        if parent_path(blocker.path) != parent {
            return plan_error("clearance blockers must share the moving prim's authored parent");
        }
        if !blocker.shape.starts_with(blocker.path + "/") {
            return plan_error(blocker.shape + ": blocker shape must be an exact descendant of its prim");
        }
        let blocker_position = local_translation(doc, blocker.path);
        if !blocker_position.ok { return blocker_position; }
        let blocker_extents = assembly_audit::axis_aligned_cube_half_extents(doc, blocker.shape);
        if blocker_extents == () {
            return plan_error(blocker.shape + ": blocker shape must be a composed axis-aligned Cube");
        }
        let clearance = assembly_audit::aabb_clearance(
            translation, moving_extents, blocker_position.value, blocker_extents,
        );
        if clearance == () || clearance.margin < minimum_gap {
            return plan_error(moving_path + ": candidate overlaps or violates the "
                + minimum_gap.to_string() + " m clearance from " + blocker.path);
        }
        seen_paths.push(blocker.path);
        seen_shapes.push(blocker.shape);
    }

    let placement = place_plan(edit_target, moving_path, translation, (), ());
    placement.kind = "clearance_placement";
    placement.blockers = seen_paths;
    placement.minimum_gap = minimum_gap;
    placement
}

/// Verify every ancestor frame that affects a shared-parent placement is
/// translation-only. Collision bounds are reported in canonical stage axes;
/// comparing them after a candidate translation is valid only when the common
/// parent chain does not rotate or scale those axes.
fn translation_only_frame_chain(doc, path, owner) {
    let frame = path;
    while frame != "/" {
        let inspected = local_translation(doc, frame);
        if !inspected.ok {
            return plan_error(owner + ": " + inspected.error);
        }
        frame = parent_path(frame);
    }
    #{ ok: true }
}

/// Read a body-owned composed collision envelope through the shared query
/// surface. Primitive dimensions, compound ownership, purpose filtering, and
/// malformed-data errors remain in the existing collision_aabb owner.
fn composed_collision_bounds(doc, path, role) {
    let record = assembly_audit::collision_bounds_record(doc, path);
    if !query_ok(record) {
        let detail = "composed collision query failed";
        if record.error != () { detail = record.error; }
        return plan_error(role + " " + path + ": " + detail);
    }
    let bounds = record.collision_bounds;
    if bounds == () {
        return plan_error(role + " " + path + ": no composed collision geometry");
    }
    if bounds.frame != "canonical_stage" {
        return plan_error(role + " " + path + ": collision bounds have an unsupported frame");
    }
    let center = vector3(bounds.center, role + " collision center");
    let half_extents = vector3(bounds.half_extents, role + " collision half extents");
    if !center.ok { return center; }
    if !half_extents.ok { return half_extents; }
    for i in 0..3 {
        if !half_extents.value[i].is_finite() || half_extents.value[i] < 0.0 {
            return plan_error(role + " " + path + ": collision half extents are invalid");
        }
    }
    #{ ok: true, center: center.value, half_extents: half_extents.value }
}

/// Collect the shared frame and aggregate-bound inputs used by collision-aware
/// alignment. Both bodies must be siblings under one translation-only ancestor
/// chain so canonical-stage bounds can be converted back to one local
/// SetTranslate operation without guessing a rotation or scale.
fn collision_alignment_context(doc, moving_path, target_path, operation) {
    if doc == () || moving_path == () || target_path == () || moving_path == target_path {
        return plan_error(operation + " requires an explicit document and two distinct body paths");
    }
    let parent = parent_path(moving_path);
    if parent_path(target_path) != parent {
        return plan_error(operation + " requires bodies with the same authored parent");
    }
    let frame_chain = translation_only_frame_chain(doc, parent, operation);
    if !frame_chain.ok { return frame_chain; }
    let moving = local_translation(doc, moving_path);
    let target = local_translation(doc, target_path);
    if !moving.ok { return moving; }
    if !target.ok { return target; }
    let moving_bounds = composed_collision_bounds(doc, moving_path, "moving body");
    let target_bounds = composed_collision_bounds(doc, target_path, "target body");
    if !moving_bounds.ok { return moving_bounds; }
    if !target_bounds.ok { return target_bounds; }
    #{
        ok: true,
        parent: parent,
        moving: moving,
        target: target,
        moving_bounds: moving_bounds,
        target_bounds: target_bounds,
    }
}

/// Align the aggregate collision-envelope centres of two sibling bodies on one
/// axis. The offset is measured from the target centre in the shared authored
/// parent frame; all other authored translation components are preserved.
fn align_collision_centers_plan(doc, edit_target, moving_path, target_path,
    axis, offset) {
    if edit_target == () {
        return plan_error("align_collision_centers_plan requires an explicit edit target");
    }
    if !cardinal_axis(axis) {
        return plan_error("align_collision_centers_plan axis must be X, Y, or Z");
    }
    if offset == () || !offset.is_finite() {
        return plan_error("align_collision_centers_plan requires a finite offset");
    }
    let context = collision_alignment_context(
        doc, moving_path, target_path, "align_collision_centers_plan",
    );
    if !context.ok { return context; }
    let index = axis_index(axis);
    let value = context.moving.value;
    value[index] += context.target_bounds.center[index] + offset
        - context.moving_bounds.center[index];
    let placement = place_plan(edit_target, moving_path, value, (), ());
    placement.kind = "collision_center_alignment";
    placement.reference = target_path;
    placement.axis = axis;
    placement.offset = offset;
    placement.collision_frame = "canonical_stage";
    placement
}

/// Align one aggregate collision-envelope edge to another sibling body on one
/// axis. Edge signs are -1 or +1 relative to each envelope centre and `gap`
/// is placed outward from the target edge. This uses the composed AABB only;
/// it does not invent a primitive-specific dimension or rotate a body.
fn align_collision_edges_plan(doc, edit_target, moving_path, target_path,
    axis, moving_edge, target_edge, gap) {
    if edit_target == () {
        return plan_error("align_collision_edges_plan requires an explicit edit target");
    }
    if !cardinal_axis(axis) {
        return plan_error("align_collision_edges_plan axis must be X, Y, or Z");
    }
    if moving_edge != -1 && moving_edge != 1 {
        return plan_error("align_collision_edges_plan moving_edge must be -1 or +1");
    }
    if target_edge != -1 && target_edge != 1 {
        return plan_error("align_collision_edges_plan target_edge must be -1 or +1");
    }
    if gap == () || !gap.is_finite() || gap < 0.0 {
        return plan_error("align_collision_edges_plan requires a finite non-negative gap");
    }
    let context = collision_alignment_context(
        doc, moving_path, target_path, "align_collision_edges_plan",
    );
    if !context.ok { return context; }
    let index = axis_index(axis);
    let target_edge_center = context.target_bounds.center[index]
        + target_edge * context.target_bounds.half_extents[index]
        + target_edge * gap;
    let candidate_center = target_edge_center
        - moving_edge * context.moving_bounds.half_extents[index];
    let value = context.moving.value;
    value[index] += candidate_center - context.moving_bounds.center[index];
    let placement = place_plan(edit_target, moving_path, value, (), ());
    placement.kind = "collision_edge_alignment";
    placement.reference = target_path;
    placement.axis = axis;
    placement.moving_edge = moving_edge;
    placement.target_edge = target_edge;
    placement.gap = gap;
    placement.collision_frame = "canonical_stage";
    placement
}

/// Place a translation-only body using its aggregate composed collision bounds
/// and the same bounds reader for every blocker. The caller supplies exact body
/// paths; each blocker is { path: "/Assembly/Obstacle" }. All bodies share one
/// authored parent and every ancestor frame is translation-only, so the
/// candidate shift can be applied to the canonical-stage moving AABB without
/// inventing a second transform or primitive-geometry implementation.
fn place_with_collision_clearance_plan(doc, edit_target, moving_path,
    translation, blockers, minimum_gap) {
    if doc == () || edit_target == () || moving_path == ()
        || blockers == () || blockers.len() == 0 {
        return plan_error("place_with_collision_clearance_plan requires a document, moving body, and explicit blockers");
    }
    if minimum_gap == () || !minimum_gap.is_finite() || minimum_gap < 0.0 {
        return plan_error("place_with_collision_clearance_plan requires a finite non-negative minimum gap");
    }
    let valid_translation = vector3(translation, "candidate translation");
    if !valid_translation.ok { return valid_translation; }
    for value in translation {
        if !value.is_finite() {
            return plan_error("candidate translation must contain finite components");
        }
    }

    let parent = parent_path(moving_path);
    let frame_chain = translation_only_frame_chain(doc, parent, moving_path);
    if !frame_chain.ok { return frame_chain; }
    let moving = local_translation(doc, moving_path);
    if !moving.ok { return moving; }
    let moving_bounds = composed_collision_bounds(doc, moving_path, "moving body");
    if !moving_bounds.ok { return moving_bounds; }

    let candidate_center = [
        moving_bounds.center[0] + translation[0] - moving.value[0],
        moving_bounds.center[1] + translation[1] - moving.value[1],
        moving_bounds.center[2] + translation[2] - moving.value[2],
    ];
    let seen_paths = [];
    for blocker in blockers {
        if blocker == () || blocker.path == () {
            return plan_error("each collision-clearance blocker requires an exact body path");
        }
        if blocker.path == moving_path || seen_paths.contains(blocker.path) {
            return plan_error("collision-clearance blockers must be unique and distinct from the moving body");
        }
        if parent_path(blocker.path) != parent {
            return plan_error("collision-clearance blockers must share the moving body's authored parent");
        }
        let blocker_position = local_translation(doc, blocker.path);
        if !blocker_position.ok { return blocker_position; }
        let blocker_bounds = composed_collision_bounds(doc, blocker.path, "blocker body");
        if !blocker_bounds.ok { return blocker_bounds; }
        let clearance = assembly_audit::aabb_clearance(
            candidate_center, moving_bounds.half_extents,
            blocker_bounds.center, blocker_bounds.half_extents,
        );
        if clearance == () || clearance.margin < minimum_gap {
            return plan_error(moving_path + ": candidate overlaps or violates the "
                + minimum_gap.to_string() + " m collision clearance from " + blocker.path);
        }
        seen_paths.push(blocker.path);
    }

    let placement = place_plan(edit_target, moving_path, translation, (), ());
    placement.kind = "collision_clearance_placement";
    placement.blockers = seen_paths;
    placement.minimum_gap = minimum_gap;
    placement.collision_frame = "canonical_stage";
    placement.candidate_center = candidate_center;
    placement
}
