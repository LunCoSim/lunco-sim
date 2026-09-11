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

/// Build a generic multi-parameter edit for one exact prim.
///
/// `parameters` is an array of `{ name, type_name, value }` maps. `value` is
/// the canonical USD literal string, so this helper supports scalar and
/// structured properties without inventing a second value codec. The helper
/// only validates the plan and returns typed `SetAttribute` operations; callers
/// submit them through `assembly_edit::batch` or `assembly_edit::propose`, the
/// same compound document boundary used by the Inspector and API.
fn parameter_plan(edit_target, path, parameters) {
    if edit_target == () || path == () || parameters == () {
        return plan_error("parameter_plan requires an edit target, exact prim path, and parameter list");
    }
    if !path.starts_with("/") || path.ends_with("/") && path != "/" {
        return plan_error("parameter_plan path must be an absolute USD prim path");
    }
    if parameters.len() == 0 {
        return plan_error("parameter_plan requires at least one parameter");
    }

    let names = [];
    let ops = [];
    for parameter in parameters {
        if parameter == () || parameter.name == () || parameter.type_name == ()
            || parameter.value == () || parameter.name == ""
            || parameter.type_name == "" {
            return plan_error(path + ": parameters require name, type_name, and value");
        }
        if names.contains(parameter.name) {
            return plan_error(path + ": parameter names must be unique");
        }
        names.push(parameter.name);
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target,
            path: path,
            name: parameter.name,
            type_name: parameter.type_name,
            value: parameter.value,
        }});
    }
    #{ ok: true, kind: "parameter_edit", path: path, parameters: names, ops: ops }
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
        doc_id: doc,
        path: parent,
        children: false,
    });
    if !query_ok(parent_record) {
        return plan_error(parent + ": referenced instance parent is unavailable");
    }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let existing = query("QueryUsdPrim", #{
        doc_id: doc,
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
    let record = query("QueryUsdPrim", #{ doc_id: doc, path: path, children: false });
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

// ── Generic component bundles ──────────────────────────────────────────────

fn bundle_plan_error(message) {
    #{ ok: false, error: message, ops: [] }
}

fn bundle_identifier(value, label) {
    if value == () || value == "" || value.contains("/") || value.contains(":") {
        return bundle_plan_error(label + " must be one USD identifier without `/` or `:`");
    }
    #{ ok: true, value: value }
}

fn bundle_vector3(value, label, positive) {
    if value == () || value.len() != 3 {
        return bundle_plan_error(label + " must have three components");
    }
    for component in value {
        if !component.is_finite() || positive && component <= 0.0 {
            let rule = if positive { " finite positive" } else { " finite" };
            return bundle_plan_error(label + " must contain" + rule + " components");
        }
    }
    #{ ok: true, value: value }
}

fn bundle_vector2(value, label, ordered) {
    if value == () || value.len() != 2 {
        return bundle_plan_error(label + " must have two components");
    }
    for component in value {
        if !component.is_finite() {
            return bundle_plan_error(label + " must contain finite components");
        }
    }
    if ordered && value[0] > value[1] {
        return bundle_plan_error(label + " must be ordered from lower to upper");
    }
    #{ ok: true, value: value }
}

fn bundle_tuple2_literal(value) {
    if value == () || value.len() != 2 { return (); }
    "(" + value[0].to_string() + ", " + value[1].to_string() + ")"
}

fn bundle_token_literal(value) {
    "\"" + value + "\""
}

fn bundle_port_name(direction, name) {
    if direction == "input" { return "inputs:" + name; }
    if direction == "output" { return "outputs:" + name; }
    ()
}

fn bundle_value_matches(actual, expected) {
    if actual == () || expected == () { return actual == expected; }
    if type_of(expected) == "array" {
        if type_of(actual) != "array" || actual.len() != expected.len() { return false; }
        for i in 0..expected.len() {
            if (actual[i] - expected[i]).abs() > 0.000001 { return false; }
        }
        return true;
    }
    if type_of(expected) != "string" && type_of(expected) != "bool" {
        return (actual - expected).abs() <= 0.000001;
    }
    actual == expected
}

fn bundle_optional_transform(spec, path) {
    let translation = if spec.translation == () { () } else { spec.translation };
    let rotation = if spec.rotation == () { () } else { spec.rotation };
    let scale = if spec.scale == () { () } else { spec.scale };
    if translation != () {
        let valid = bundle_vector3(translation, path + " translation", false);
        if !valid.ok { return valid; }
    }
    if rotation != () {
        let valid = bundle_vector3(rotation, path + " rotation", false);
        if !valid.ok { return valid; }
    }
    if scale != () {
        let valid = bundle_vector3(scale, path + " scale", true);
        if !valid.ok { return valid; }
    }
    #{ ok: true, translation: translation, rotation: rotation, scale: scale }
}

/// Normalize one reusable component contract before any USD operation is
/// proposed. Dimensions are SI metres. `geometry` is one list so a shape can
/// explicitly be visual, collision-only, or both without duplicating a path.
/// `frames` and `actuators` are named contracts, not vehicle-specific schemas.
fn component_bundle_facts(bundle) {
    if bundle == () {
        return bundle_plan_error("component_bundle_facts requires a bundle map");
    }
    if bundle.units != "m" {
        return bundle_plan_error("component bundle units must be explicit SI metres (`m`)");
    }
    let dimensions = bundle_vector3(bundle.dimensions,
        "component bundle dimensions", true);
    if !dimensions.ok { return dimensions; }
    if bundle.geometry == () || bundle.geometry.len() == 0 {
        return bundle_plan_error("component bundle requires at least one geometry entry");
    }

    let names = [];
    let geometry = [];
    let visual = [];
    let collision = [];
    for spec in bundle.geometry {
        let identity = bundle_identifier(spec.name, "geometry name");
        if !identity.ok { return identity; }
        if names.contains(spec.name) {
            return bundle_plan_error("component bundle identity is duplicated: " + spec.name);
        }
        names.push(spec.name);
        if spec.kind != "Cube" && spec.kind != "Cylinder" && spec.kind != "Cone" {
            return bundle_plan_error(spec.name + ": geometry kind must be Cube, Cylinder, or Cone");
        }
        let render = if spec.render == () { true } else { spec.render };
        let collide = if spec.collision == () { false } else { spec.collision };
        if !render && !collide {
            return bundle_plan_error(spec.name + ": geometry must render, collide, or both");
        }
        let transform = bundle_optional_transform(spec, spec.name);
        if !transform.ok { return transform; }
        let shape = #{
            name: spec.name,
            kind: spec.kind,
            render: render,
            collision: collide,
            translation: transform.translation,
            rotation: transform.rotation,
            scale: transform.scale,
            material: if spec.material == () { () } else { spec.material },
        };
        if spec.kind == "Cube" {
            let size = if spec.size == () { () } else { spec.size };
            if size == () || !size.is_finite() || size <= 0.0 {
                return bundle_plan_error(spec.name + ": Cube requires a finite positive size");
            }
            shape.size = size;
        } else {
            let radius = if spec.radius == () { () } else { spec.radius };
            let height = if spec.height == () { () } else { spec.height };
            if radius == () || height == () || !radius.is_finite() || !height.is_finite()
                || radius <= 0.0 || height <= 0.0 {
                return bundle_plan_error(spec.name + ": " + spec.kind + " requires finite positive radius and height");
            }
            shape.radius = radius;
            shape.height = height;
        }
        if spec.axis != () && !cardinal_axis(spec.axis) {
            return bundle_plan_error(spec.name + ": axis must be X, Y, or Z");
        }
        shape.axis = if spec.axis == () { () } else { spec.axis };
        if shape.material != () && (!shape.material.starts_with("/") || shape.material.ends_with("/")) {
            return bundle_plan_error(spec.name + ": material must be an absolute USD prim path");
        }
        geometry.push(shape);
        if render { visual.push(spec.name); }
        if collide { collision.push(spec.name); }
    }

    let frames = [];
    let mount = ();
    if bundle.frames != () {
        for spec in bundle.frames {
            let identity = bundle_identifier(spec.name, "frame name");
            if !identity.ok { return identity; }
            if names.contains(spec.name) {
                return bundle_plan_error("component bundle identity is duplicated: " + spec.name);
            }
            names.push(spec.name);
            if spec.role != "datum" && spec.role != "attachment" && spec.role != "actuator" {
                return bundle_plan_error(spec.name + ": frame role must be datum, attachment, or actuator");
            }
            let transform = bundle_optional_transform(spec, spec.name);
            if !transform.ok { return transform; }
            let frame = #{
                name: spec.name,
                role: spec.role,
                translation: transform.translation,
                rotation: transform.rotation,
                scale: transform.scale,
                mount_kind: if spec.mount_kind == () { () } else { spec.mount_kind },
            };
            if frame.mount_kind != () {
                if spec.role != "attachment" || frame.mount_kind == "" {
                    return bundle_plan_error(spec.name + ": mount_kind is only valid on a named attachment frame");
                }
                let mount_identity = bundle_identifier(frame.mount_kind, "mount kind");
                if !mount_identity.ok { return mount_identity; }
                if mount != () {
                    return bundle_plan_error("component bundle may advertise only one standard mount plug");
                }
                mount = frame;
            }
            frames.push(frame);
        }
    }

    let actuators = [];
    if bundle.actuators != () {
        let actuator_names = [];
        for spec in bundle.actuators {
            let identity = bundle_identifier(spec.name, "actuator name");
            if !identity.ok { return identity; }
            if actuator_names.contains(spec.name) {
                return bundle_plan_error("component actuator is duplicated: " + spec.name);
            }
            actuator_names.push(spec.name);
            if spec.direction != "input" && spec.direction != "output" {
                return bundle_plan_error(spec.name + ": actuator direction must be input or output");
            }
            if spec.unit == () || spec.unit == "" {
                return bundle_plan_error(spec.name + ": actuator unit is required");
            }
            if spec.frame == () || spec.frame == "" {
                return bundle_plan_error(spec.name + ": actuator requires an explicit frame");
            }
            let frame_found = false;
            for frame in frames {
                if frame.name == spec.frame {
                    frame_found = true;
                    if frame.role != "actuator" {
                        return bundle_plan_error(spec.name + ": actuator frame must have role actuator");
                    }
                }
            }
            if !frame_found {
                return bundle_plan_error(spec.name + ": actuator frame is not declared");
            }
            let default_value = if spec.default_value == () { 0.0 } else { spec.default_value };
            if !default_value.is_finite() {
                return bundle_plan_error(spec.name + ": actuator default_value must be finite");
            }
            let limits = ();
            if spec.limits != () {
                limits = bundle_vector2(spec.limits, spec.name + " limits", true);
                if !limits.ok { return limits; }
                limits = limits.value;
            }
            actuators.push(#{
                name: spec.name,
                direction: spec.direction,
                unit: spec.unit,
                frame: spec.frame,
                default_value: default_value,
                limits: limits,
            });
        }
    }

    let mass = ();
    if bundle.mass != () {
        if bundle.mass.value == () || !bundle.mass.value.is_finite() || bundle.mass.value <= 0.0 {
            return bundle_plan_error("component mass requires a finite positive value");
        }
        let center_of_mass = bundle_vector3(bundle.mass.center_of_mass,
            "component centre of mass", false);
        if !center_of_mass.ok { return center_of_mass; }
        let diagonal_inertia = bundle_vector3(bundle.mass.diagonal_inertia,
            "component diagonal inertia", true);
        if !diagonal_inertia.ok { return diagonal_inertia; }
        mass = #{
            value: bundle.mass.value,
            center_of_mass: center_of_mass.value,
            diagonal_inertia: diagonal_inertia.value,
        };
    }

    let deployment = ();
    if bundle.deployment != () {
        if bundle.deployment.state == () || bundle.deployment.state == ""
            || bundle.deployment.units == () || bundle.deployment.units == "" {
            return bundle_plan_error("deployment requires explicit state and units");
        }
        let limits = bundle_vector2(bundle.deployment.limits,
            "deployment limits", true);
        if !limits.ok { return limits; }
        deployment = #{
            state: bundle.deployment.state,
            units: bundle.deployment.units,
            limits: limits.value,
        };
    }

    #{
        ok: true,
        units: bundle.units,
        dimensions: dimensions.value,
        geometry: geometry,
        visual: visual,
        collision: collision,
        frames: frames,
        mount: mount,
        actuators: actuators,
        mass: mass,
        deployment: deployment,
        spawnable: if bundle.spawnable == () { false } else { bundle.spawnable },
    }
}

fn bundle_geometry_plan(edit_target, parent, shape) {
    let path = parent + "/" + shape.name;
    let transform = place_plan(edit_target, path, shape.translation,
        shape.rotation, shape.scale);
    if !transform.ok && (shape.translation != () || shape.rotation != () || shape.scale != ()) {
        return transform;
    }
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: shape.name,
        type_name: shape.kind,
        reference: (),
        reference_prim_path: "",
    }}];
    if transform.ok {
        for op in transform.ops { ops.push(op); }
    }
    if shape.kind == "Cube" {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "size",
            type_name: "double", value: shape.size.to_string(),
        }});
    } else {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "radius",
            type_name: "double", value: shape.radius.to_string(),
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "height",
            type_name: "double", value: shape.height.to_string(),
        }});
    }
    if shape.axis != () {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "axis",
            type_name: "token", value: bundle_token_literal(shape.axis),
        }});
    }
    if shape.collision {
        ops.push(#{ SetApiSchemas: #{
            edit_target: edit_target, path: path,
            schemas: ["PhysicsCollisionAPI"],
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path,
            name: "physics:collisionEnabled", type_name: "bool", value: "true",
        }});
    }
    if !shape.render {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path,
            name: "visibility", type_name: "token", value: "\"invisible\"",
        }});
    }
    if shape.material != () {
        ops.push(#{ SetApiSchemas: #{
            edit_target: edit_target, path: path,
            schemas: ["MaterialBindingAPI"],
        }});
        ops.push(#{ SetRelationship: #{
            edit_target: edit_target, path: path,
            name: "material:binding", targets: [shape.material],
        }});
    }
    #{ ok: true, path: path, ops: ops }
}

/// Build one generic parametric component bundle as a pure typed USD plan.
/// USD owns prim identity, `kind`, transforms, typed geometry properties,
/// physics mass, material binding, and `inputs:`/`outputs:` ports. The bundle's
/// dimensions, actuator units/limits, and deployment state remain caller-side
/// draft data until a standard geometry, joint, variant, or Modelica declaration
/// owns them. The caller submits the returned `.ops` through one reviewed
/// `assembly_edit` proposal or batch. No vehicle identity is encoded here.
fn component_bundle_plan(edit_target, parent, name, bundle) {
    if edit_target == () || parent == () || name == () {
        return bundle_plan_error("component_bundle_plan requires edit target, parent path, name, and bundle");
    }
    if !parent.starts_with("/") || parent.ends_with("/") && parent != "/" {
        return bundle_plan_error("component_bundle_plan parent must be an absolute USD path");
    }
    let identity = bundle_identifier(name, "component bundle name");
    if !identity.ok { return identity; }
    let facts = component_bundle_facts(bundle);
    if !facts.ok { return facts; }
    let path = if parent == "/" { "/" + name } else { parent + "/" + name };
    let schemas = ["LunCoCatalogAPI"];
    if facts.mass != () { schemas.push("PhysicsMassAPI"); }
    if facts.mount != () { schemas.push("LunCoMountPlugAPI"); }
    let ops = [#{ AddPrim: #{
        edit_target: edit_target,
        parent_path: parent,
        name: name,
        type_name: "Xform",
        reference: (),
        reference_prim_path: "",
    }}];
    ops.push(#{ SetPrimKind: #{ edit_target: edit_target, path: path, kind: "component" }});
    ops.push(#{ SetApiSchemas: #{ edit_target: edit_target, path: path, schemas: schemas }});
    if facts.spawnable {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "lunco:spawnable",
            type_name: "bool", value: "true",
        }});
    }
    if facts.mass != () {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "physics:mass",
            type_name: "float", value: facts.mass.value.to_string(),
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "physics:centerOfMass",
            type_name: "point3f", value: assembly_edit::tuple3_literal(facts.mass.center_of_mass),
        }});
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "physics:diagonalInertia",
            type_name: "float3", value: assembly_edit::tuple3_literal(facts.mass.diagonal_inertia),
        }});
    }
    if facts.mount != () {
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: "lunco:mount:plug",
            type_name: "token", value: bundle_token_literal(facts.mount.mount_kind),
        }});
        ops.push(#{ SetRelationship: #{
            edit_target: edit_target, path: path, name: "lunco:mount:frame",
            targets: [path + "/" + facts.mount.name],
        }});
    }
    let frame_paths = [];
    for frame in facts.frames {
        let planned = frame_plan(edit_target, path, frame.name,
            frame.translation, frame.rotation, frame.scale);
        if !planned.ok { return planned; }
        for op in planned.ops { ops.push(op); }
        let frame_path = path + "/" + frame.name;
        frame_paths.push(frame_path);
    }
    for actuator in facts.actuators {
        let port_name = bundle_port_name(actuator.direction, actuator.name);
        if port_name == () {
            return bundle_plan_error("actuator direction must map to a USD inputs:/outputs: port");
        }
        ops.push(#{ SetAttribute: #{
            edit_target: edit_target, path: path, name: port_name,
            type_name: "float", value: actuator.default_value.to_string(),
        }});
    }
    let visual_paths = [];
    let collision_paths = [];
    for shape in facts.geometry {
        let planned = bundle_geometry_plan(edit_target, path, shape);
        if !planned.ok { return planned; }
        for op in planned.ops { ops.push(op); }
        if shape.render { visual_paths.push(planned.path); }
        if shape.collision { collision_paths.push(planned.path); }
    }
    #{
        ok: true,
        kind: "component_bundle",
        path: path,
        contract: facts,
        visual_paths: visual_paths,
        collision_paths: collision_paths,
        frame_paths: frame_paths,
        actuator_paths: facts.actuators.map(|actuator| path + "/" + actuator.frame),
        input_ports: facts.actuators.map(|actuator| bundle_port_name(actuator.direction, actuator.name)),
        ops: ops,
    }
}

fn bundle_transform_update_ops(edit_target, path, record, translation, rotation, scale) {
    let ops = [];
    let attrs = if record.attrs == () { #{} } else { record.attrs };
    if translation != () && !bundle_value_matches(attrs["xformOp:translate"], translation) {
        ops.push(#{ SetTranslate: #{
            edit_target: edit_target, path: path, value: translation,
        }});
    }
    if rotation != () && !bundle_value_matches(attrs["xformOp:rotateXYZ"], rotation) {
        ops.push(#{ SetRotate: #{
            edit_target: edit_target, path: path, value: rotation,
        }});
    }
    if scale != () && !bundle_value_matches(attrs["xformOp:scale"], scale) {
        ops.push(#{ SetScale: #{
            edit_target: edit_target, path: path, value: scale,
        }});
    }
    ops
}

/// Build a regeneration plan for one already-authored generic component.
///
/// The caller supplies the new component draft and optional explicit
/// Modelica/USD bindings. Existing geometry and frame identities must still
/// exist with the same primitive kinds; extra authored children and material
/// bindings are preserved. The plan changes only standard USD geometry,
/// transforms, physics mass properties, and explicit typed bindings. Bundle
/// dimensions are caller-side draft data; deployment state and actuator limits
/// belong to standard variants/joints or Modelica declarations. An unchanged
/// draft returns an empty operation list so the caller can skip the journal.
fn component_bundle_update_plan(doc, edit_target, path, bundle, bindings) {
    if doc == () || edit_target == () || path == () || bundle == () {
        return bundle_plan_error(
            "component_bundle_update_plan requires document, edit target, exact component path, and bundle",
        );
    }
    if !path.starts_with("/") || path.ends_with("/") {
        return bundle_plan_error("component_bundle_update_plan path must be an absolute USD prim path");
    }
    let facts = component_bundle_facts(bundle);
    if !facts.ok { return facts; }
    let root = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: [
            "lunco:spawnable", "physics:mass", "physics:centerOfMass",
            "physics:diagonalInertia", "lunco:mount:plug",
        ],
        rels: ["lunco:mount:frame"],
        children: true,
    });
    if !query_ok(root) || root.type_name != "Xform" {
        return bundle_plan_error(path + ": component root is unavailable or not an Xform");
    }

    let ops = [];
    let root_attrs = root.attrs;
    if facts.mass != () {
        if !bundle_value_matches(root_attrs["physics:mass"], facts.mass.value) {
            ops.push(#{ SetAttribute: #{
                edit_target: edit_target, path: path, name: "physics:mass",
                type_name: "float", value: facts.mass.value.to_string(),
            }});
        }
        if !bundle_value_matches(root_attrs["physics:centerOfMass"], facts.mass.center_of_mass) {
            ops.push(#{ SetAttribute: #{
                edit_target: edit_target, path: path,
                name: "physics:centerOfMass", type_name: "point3f",
                value: assembly_edit::tuple3_literal(facts.mass.center_of_mass),
            }});
        }
        if !bundle_value_matches(root_attrs["physics:diagonalInertia"], facts.mass.diagonal_inertia) {
            ops.push(#{ SetAttribute: #{
                edit_target: edit_target, path: path,
                name: "physics:diagonalInertia", type_name: "float3",
                value: assembly_edit::tuple3_literal(facts.mass.diagonal_inertia),
            }});
        }
    }
    for frame in facts.frames {
        let frame_path = path + "/" + frame.name;
        let record = query("QueryUsdPrim", #{
            doc_id: doc,
            path: frame_path,
            attrs: ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"],
            children: false,
        });
        if !query_ok(record) || record.type_name != "Xform" {
            return bundle_plan_error(frame_path + ": regeneration cannot change frame topology or type");
        }
        let transform_ops = bundle_transform_update_ops(edit_target, frame_path, record,
            frame.translation, frame.rotation, frame.scale);
        for op in transform_ops { ops.push(op); }
    }

    for actuator in facts.actuators {
        let frame_path = path + "/" + actuator.frame;
        let frame = query("QueryUsdPrim", #{
            doc_id: doc,
            path: frame_path,
            children: false,
        });
        if !query_ok(frame) || frame.type_name != "Xform" {
            return bundle_plan_error(frame_path + ": regeneration cannot change actuator frame topology or type");
        }
        let port_name = bundle_port_name(actuator.direction, actuator.name);
        if port_name == () {
            return bundle_plan_error("actuator direction must map to a USD inputs:/outputs: port");
        }
        let port = query("QueryUsdPrim", #{
            doc_id: doc, path: path, attrs: [port_name], children: false,
        });
        if !query_ok(port) {
            return bundle_plan_error(path + ": actuator port is unavailable: " + port_name);
        }
        if !bundle_value_matches(port.attrs[port_name], actuator.default_value) {
            ops.push(#{ SetAttribute: #{
                edit_target: edit_target, path: path, name: port_name,
                type_name: "float", value: actuator.default_value.to_string(),
            }});
        }
    }

    for shape in facts.geometry {
        let shape_path = path + "/" + shape.name;
        let record = query("QueryUsdPrim", #{
            doc_id: doc,
            path: shape_path,
            attrs: ["size", "radius", "height", "axis",
                "physics:collisionEnabled", "visibility", "xformOp:translate",
                "xformOp:rotateXYZ", "xformOp:scale"],
            rels: ["material:binding"],
            children: false,
        });
        if !query_ok(record) || record.type_name != shape.kind {
            return bundle_plan_error(shape_path + ": regeneration cannot change geometry topology or kind");
        }
        let collision_enabled = record.attrs["physics:collisionEnabled"];
        if shape.collision && collision_enabled != true {
            return bundle_plan_error(shape_path + ": collision role drifted; regeneration will not invent a collider");
        }
        if !shape.collision && collision_enabled == true {
            return bundle_plan_error(shape_path + ": collision role drifted; regeneration will not remove a collider");
        }
        let visibility = record.attrs["visibility"];
        if shape.render && visibility == "invisible" {
            return bundle_plan_error(shape_path + ": visual role drifted; regeneration will not unhide authored geometry");
        }
        if !shape.render && visibility != "invisible" {
            return bundle_plan_error(shape_path + ": visual role drifted; regeneration will not hide authored geometry");
        }
        if shape.material != () {
            let targets = record.relationships["material:binding"];
            if targets == () || targets.len() != 1 || targets[0] != shape.material {
                return bundle_plan_error(shape_path + ": material binding drifted; regeneration preserves material ownership");
            }
        }
        let transform_ops = bundle_transform_update_ops(edit_target, shape_path, record,
            shape.translation, shape.rotation, shape.scale);
        for op in transform_ops { ops.push(op); }
        if shape.kind == "Cube" {
            if !bundle_value_matches(record.attrs["size"], shape.size) {
                ops.push(#{ SetAttribute: #{
                    edit_target: edit_target, path: shape_path, name: "size",
                    type_name: "double", value: shape.size.to_string(),
                }});
            }
        } else {
            if !bundle_value_matches(record.attrs["radius"], shape.radius) {
                ops.push(#{ SetAttribute: #{
                    edit_target: edit_target, path: shape_path, name: "radius",
                    type_name: "double", value: shape.radius.to_string(),
                }});
            }
            if !bundle_value_matches(record.attrs["height"], shape.height) {
                ops.push(#{ SetAttribute: #{
                    edit_target: edit_target, path: shape_path, name: "height",
                    type_name: "double", value: shape.height.to_string(),
                }});
            }
        }
        if shape.axis != () && !bundle_value_matches(record.attrs["axis"], shape.axis) {
            ops.push(#{ SetAttribute: #{
                edit_target: edit_target, path: shape_path, name: "axis",
                type_name: "token", value: bundle_token_literal(shape.axis),
            }});
        }
    }

    let binding_names = [];
    if bindings != () {
        for binding in bindings {
            if binding == () || binding.path == () || binding.name == ()
                || binding.type_name == () || binding.value == ()
                || binding.expected == () || !binding.path.starts_with("/")
                || binding.name == "" || binding.type_name == ""
                || binding_names.contains(binding.path + ":" + binding.name) {
                return bundle_plan_error(path + ": bindings require unique absolute path, name, type_name, value, and expected");
            }
            let binding_prim = query("QueryUsdPrim", #{
                doc_id: doc, path: binding.path, attrs: [binding.name], children: false,
            });
            if !query_ok(binding_prim) {
                return bundle_plan_error(binding.path + ": explicit binding target is unavailable");
            }
            binding_names.push(binding.path + ":" + binding.name);
            if !bundle_value_matches(binding_prim.attrs[binding.name], binding.expected) {
                ops.push(#{ SetAttribute: #{
                    edit_target: edit_target, path: binding.path, name: binding.name,
                    type_name: binding.type_name, value: binding.value.to_string(),
                }});
            }
        }
    }
    #{
        ok: true,
        kind: "component_bundle_update",
        path: path,
        topology_preserved: true,
        bindings: binding_names,
        no_op: ops.len() == 0,
        ops: ops,
    }
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
        doc_id: doc,
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
            doc_id: doc,
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

/// Return one structured authoring record for an exact composed prim.
///
/// This is the discovery surface for both an Editor assistant and a human
/// tool: it keeps the document handle, generation, legal edit target, exact
/// prim facts, topology, collision envelope, and authored mount affordances
/// together. It never infers a component from a name and never mutates USD.
/// Callers use the returned paths to build one of the typed plans below.
fn authoring_context(doc, path, edit_target) {
    if doc == () || path == () || edit_target == () {
        return plan_error("authoring_context requires document, exact prim path, and edit target");
    }
    if !path.starts_with("/") || path.ends_with("/") && path != "/" {
        return plan_error("authoring_context path must be an absolute USD prim path");
    }

    let document = assembly_edit::describe(doc);
    if !query_ok(document) || document.generation == () {
        return plan_error("authoring_context could not read the document generation");
    }
    let target = assembly_edit::resolve_target(doc, path, edit_target);
    if !query_ok(target) {
        return plan_error(path + ": authored edit target is unavailable");
    }
    let prim = query("QueryUsdPrim", #{
        doc_id: doc,
        path: path,
        attrs: [
            "xformOp:translate",
            "xformOp:rotateXYZ",
            "xformOp:scale",
            "xformOpOrder",
            "lunco:mount:plug",
            "physics:mass",
            "physics:collisionEnabled",
        ],
        rels: [
            "lunco:mount:sockets",
            "lunco:mount:frame",
            "lunco:mount:attachmentJoint",
            "lunco:mount:part",
        ],
        topology: true,
        collision_bounds: true,
        children: false,
    });
    if !query_ok(prim) {
        return plan_error(path + ": composed prim is unavailable");
    }

    let sockets = [];
    let socket_paths = mount_relationship_targets(prim, "lunco:mount:sockets");
    for socket_path in socket_paths {
        let socket = query("QueryUsdPrim", #{
            doc_id: doc,
            path: socket_path,
            attrs: [
                "lunco:mount:socket",
                "lunco:mount:joint",
                "lunco:mount:axis",
                "lunco:mount:asset",
            ],
            rels: ["lunco:mount:part"],
        });
        let socket_available = query_ok(socket) && socket.attrs != ();
        let occupied = mount_relationship_targets(socket, "lunco:mount:part");
        sockets.push(#{
            path: socket_path,
            available: socket_available,
            kind: if socket_available { socket.attrs["lunco:mount:socket"] } else { () },
            joint: if socket_available { socket.attrs["lunco:mount:joint"] } else { () },
            axis: if socket_available { socket.attrs["lunco:mount:axis"] } else { () },
            asset: if socket_available { socket.attrs["lunco:mount:asset"] } else { () },
            occupied_by: occupied,
            empty: occupied.len() == 0,
        });
    }

    let frame_paths = mount_relationship_targets(prim, "lunco:mount:frame");
    let joint_paths = mount_relationship_targets(prim, "lunco:mount:attachmentJoint");
    let plug_kind = if prim.attrs == () { () } else { prim.attrs["lunco:mount:plug"] };
    let plug = if plug_kind == () && frame_paths.len() == 0 && joint_paths.len() == 0 {
        ()
    } else {
        #{ kind: plug_kind, frame_paths: frame_paths, joint_paths: joint_paths }
    };
    let can_realign = plug_kind != () && frame_paths.len() == 1 && joint_paths.len() == 1;
    let actions = ["inspect", "transform"];
    if socket_paths.len() != 0 { actions.push("plan_component_attachment"); }
    if can_realign { actions.push("plan_existing_mount_realignment"); }
    #{
        ok: true,
        kind: "authoring_context",
        doc_id: doc,
        path: path,
        parent_path: parent_path(path),
        edit_target: edit_target,
        generation: document.generation,
        target: target,
        prim: prim,
        topology: prim.topology,
        collision_bounds: prim.collision_bounds,
        sockets: sockets,
        plug: plug,
        actions: actions,
    }
}

/// Lift the focused Editor selection into the same exact authoring record used
/// by explicit-path tools. The selection query remains the identity owner: a
/// missing, multi-selected, stale, or ambiguous selection is an actionable
/// failure rather than an invitation to guess a path or document.
fn selected_authoring_context(preview) {
    let selection = if preview == () {
        assembly_edit::selection_context()
    } else {
        assembly_edit::selection_context_for(preview)
    };
    if !query_ok(selection) {
        return plan_error("selected_authoring_context could not read Editor selection");
    }
    if selection.selection_state != "single" || selection.primary == () {
        return plan_error("selected_authoring_context requires exactly one selected prim");
    }
    if selection.stale_selection_count != () && selection.stale_selection_count != 0 {
        return plan_error("selected_authoring_context rejects stale selection entries");
    }
    if selection.ambiguous_paths != () && selection.ambiguous_paths.len() != 0 {
        return plan_error("selected_authoring_context rejects ambiguous selection paths");
    }
    if selection.edit_target == () {
        return plan_error("selected_authoring_context selection has no edit target");
    }
    let doc = if selection.doc_id != () { selection.doc_id } else { selection.doc };
    let path = selection.primary.path;
    if doc == () || path == () {
        return plan_error("selected_authoring_context selection has no document or prim path");
    }
    let context = authoring_context(doc, path, selection.edit_target);
    if !context.ok { return context; }
    context.kind = "selected_authoring_context";
    context.selection = selection;
    context.selection_identity = selection.primary.identity;
    context
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
        doc_id: doc,
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
            doc_id: doc,
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

/// Build a dry component-attachment plan from explicit authored socket facts.
/// The returned `spec` is consumed by `assembly_edit::attach_component` only
/// after review; this function never creates a prim, reference, joint, or
/// occupancy relationship.
fn mount_component_plan(doc, edit_target, host_path, socket_kind, joint_kind,
    asset, name, joint_name) {
    if doc == () || edit_target == () || name == () || joint_name == () {
        return plan_error("mount_component_plan requires document, edit target, component name, and joint name");
    }
    let selected = find_compatible_socket(doc, host_path, socket_kind,
        joint_kind, asset, true);
    if !selected.ok { return selected; }

    let spec = #{
        edit_target: edit_target,
        socket_path: selected.socket_path,
        host_path: host_path,
        name: name,
        joint_name: joint_name,
        asset: asset,
        placement: [0.0, 0.0, 0.0],
        rotate_deg: [0.0, 0.0, 0.0],
        joint: selected.joint,
    };
    #{
        ok: true,
        kind: "component_attachment_plan",
        mode: "attach_component",
        host_path: host_path,
        socket_path: selected.socket_path,
        asset: asset,
        moved: [if host_path == "/" { "/" + name } else { host_path + "/" + name }],
        fixed: [host_path, selected.socket_path],
        mutation: "assembly_edit::attach_component",
        spec: spec,
        ops: [],
    }
}

/// Attach a reusable component after its explicit dry plan has been reviewed.
/// All lowering, frame resolution, validation, occupancy, and journalling
/// remain in the existing generic AttachComponent owner.
fn mount_component(doc, edit_target, host_path, socket_kind, joint_kind,
    asset, name, joint_name) {
    let plan = mount_component_plan(doc, edit_target, host_path, socket_kind,
        joint_kind, asset, name, joint_name);
    if !plan.ok { return plan; }

    let result = assembly_edit::attach_component(doc, plan.spec);
    if result.ok {
        result.kind = "socket_mount";
        result.socket_path = plan.socket_path;
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
        doc_id: doc,
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

/// Discover the standard mount frame explicitly advertised by one component or
/// assembly root. The relationship is the source of truth; the helper never
/// scans child names or treats geometry as a frame by convention. Generic
/// datum/actuator frames are returned by the component plan to its caller and
/// are later consumed by an explicit standard joint or Modelica port contract.
fn functional_frame_catalog(doc, edit_target, root_path) {
    if doc == () || edit_target == () || root_path == () {
        return plan_error("functional_frame_catalog requires document, edit target, and root path");
    }
    if !root_path.starts_with("/") || root_path.ends_with("/") && root_path != "/" {
        return plan_error("functional_frame_catalog root path must be an absolute USD path");
    }
    let document = assembly_edit::describe(doc);
    if !query_ok(document) || document.generation == () {
        return plan_error("functional_frame_catalog could not read the document generation");
    }
    let target = assembly_edit::resolve_target(doc, root_path, edit_target);
    if !query_ok(target) {
        return plan_error(root_path + ": authored root target is unavailable");
    }
    let root = query("QueryUsdPrim", #{
        doc_id: doc,
        path: root_path,
        attrs: ["lunco:mount:plug"],
        rels: [
            "lunco:mount:frame",
            "lunco:mount:sockets",
        ],
        children: false,
    });
    if !query_ok(root) {
        return plan_error(root_path + ": composed root is unavailable");
    }

    let frame_paths = mount_relationship_targets(root, "lunco:mount:frame");

    let frames = [];
    for frame_path in frame_paths {
        let frame = query("QueryUsdPrim", #{
            doc_id: doc,
            path: frame_path,
            attrs: [
                "xformOp:translate",
                "xformOp:rotateXYZ",
                "xformOp:scale",
                "xformOpOrder",
                "lunco:mount:socket",
                "lunco:mount:joint",
                "lunco:mount:axis",
            ],
            rels: ["lunco:mount:part"],
            children: false,
        });
        if !query_ok(frame) {
            return plan_error(frame_path + ": advertised functional frame is unavailable");
        }
        let local = mount_local_transform(doc, frame_path);
        if !local.ok { return local; }
        let relative = mount_frame_in_body(doc, root_path, frame_path);
        if !relative.ok { return relative; }
        frames.push(#{
            path: frame_path,
            leaf_name: frame_path.split("/")[frame_path.split("/").len() - 1],
            role: "attachment",
            is_mount_frame: true,
            is_actuator_frame: false,
            local_transform: local.value,
            relative_transform: relative.value,
            mount: #{
                socket_kind: if frame.attrs == () { () } else { frame.attrs["lunco:mount:socket"] },
                joint_kind: if frame.attrs == () { () } else { frame.attrs["lunco:mount:joint"] },
                axis: if frame.attrs == () { () } else { frame.attrs["lunco:mount:axis"] },
                occupied_by: mount_relationship_targets(frame, "lunco:mount:part"),
            },
            actuator: #{ name: (), direction: (), unit: (), limits: (), ports: [] },
        });
    }
    #{
        ok: true,
        kind: "functional_frame_catalog",
        doc_id: doc,
        edit_target: edit_target,
        root_path: root_path,
        generation: document.generation,
        root: root,
        socket_paths: mount_relationship_targets(root, "lunco:mount:sockets"),
        frame_paths: frame_paths,
        frames: frames,
    }
}

/// Compute a dry rigid placement that makes one exact source frame coincide
/// with one exact target frame. The moving and target roots must be siblings;
/// this keeps the returned SetTranslate/SetRotate local to one authored parent
/// and avoids pretending to solve arbitrary USD hierarchy or scale.
fn align_frames_plan(doc, edit_target, moving_path, moving_frame_path,
    target_path, target_frame_path) {
    if doc == () || edit_target == () || moving_path == ()
        || moving_frame_path == () || target_path == () || target_frame_path == () {
        return plan_error("align_frames_plan requires document, edit target, moving root/frame, and target root/frame paths");
    }
    for path in [moving_path, moving_frame_path, target_path, target_frame_path] {
        if !path.starts_with("/") || path.ends_with("/") {
            return plan_error("align_frames_plan paths must be absolute USD paths");
        }
    }
    if moving_path == target_path {
        return plan_error("align_frames_plan requires distinct moving and target roots");
    }
    if parent_path(moving_path) != parent_path(target_path) {
        return plan_error("align_frames_plan requires moving and target roots to share the same authored parent");
    }
    let document = assembly_edit::describe(doc);
    if !query_ok(document) || document.generation == () {
        return plan_error("align_frames_plan could not read the document generation");
    }

    let moving_root = mount_local_transform(doc, moving_path);
    if !moving_root.ok { return moving_root; }
    let target_root = mount_local_transform(doc, target_path);
    if !target_root.ok { return target_root; }
    let source_frame = mount_frame_in_body(doc, moving_path, moving_frame_path);
    if !source_frame.ok { return source_frame; }
    let target_frame = mount_frame_in_body(doc, target_path, target_frame_path);
    if !target_frame.ok { return target_frame; }

    let target_in_parent = mount_transform_compose(target_root.value, target_frame.value);
    let placement = mount_transform_compose(
        target_in_parent, mount_transform_inverse(source_frame.value),
    );
    let translation = finite_vector3(placement.translation,
        "resolved frame alignment translation");
    if !translation.ok { return translation; }
    let rotation = mount_xyz_deg_from_quat(placement.rotation);
    let valid_rotation = finite_vector3(rotation, "resolved frame alignment rotation");
    if !valid_rotation.ok { return valid_rotation; }
    #{
        ok: true,
        kind: "frame_alignment_plan",
        mode: "placement",
        moving_path: moving_path,
        moving_frame_path: moving_frame_path,
        target_path: target_path,
        target_frame_path: target_frame_path,
        moved: [moving_path],
        fixed: [target_path, target_frame_path],
        generation: document.generation,
        placement: placement.translation,
        rotation: rotation,
        mutation: "assembly_edit::propose",
        ops: [
            #{ SetTranslate: #{
                edit_target: edit_target,
                path: moving_path,
                value: placement.translation,
            }},
            #{ SetRotate: #{
                edit_target: edit_target,
                path: moving_path,
                value: rotation,
            }},
        ],
    }
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
        doc_id: doc,
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
        doc_id: doc,
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
        doc_id: doc,
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
        doc_id: doc,
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

/// Plan either side of the generic placement/attachment workflow.
///
/// `request.mode == "attach_component"` returns an explicit AttachSpec for a
/// new referenced component. `request.mode == "realign_existing_mount"`
/// returns the reviewed USD operations for an already attached part. The
/// request must carry exact paths and authored identities in both cases; this
/// dispatcher only makes the two safe, dry planning surfaces discoverable to
/// an AI or a small human-facing tool.
fn place_or_attach_plan(doc, edit_target, request) {
    if doc == () || edit_target == () || request == () || request.mode == () {
        return plan_error("place_or_attach_plan requires document, edit target, and explicit mode");
    }
    if request.mode == "attach_component" {
        if request.host_path == () || request.socket_kind == ()
            || request.joint_kind == () || request.asset == ()
            || request.name == () || request.joint_name == () {
            return plan_error("attach_component planning requires host, socket kind, joint kind, asset, name, and joint name");
        }
        return mount_component_plan(
            doc, edit_target, request.host_path, request.socket_kind,
            request.joint_kind, request.asset, request.name, request.joint_name,
        );
    }
    if request.mode == "realign_existing_mount" {
        if request.host_path == () || request.socket_path == ()
            || request.part_path == () || request.joint_path == () {
            return plan_error("existing-mount planning requires host, socket, part, and joint paths");
        }
        let plan = mount_frame_realignment_plan(
            doc, edit_target, request.host_path, request.socket_path,
            request.part_path, request.joint_path,
        );
        if !plan.ok { return plan; }
        plan.kind = "placement_attachment_plan";
        plan.mode = request.mode;
        plan.moved = [request.part_path];
        plan.fixed = [request.host_path, request.socket_path, request.joint_path];
        plan.mutation = "assembly_edit::propose";
        return plan;
    }
    plan_error("place_or_attach_plan mode must be attach_component or realign_existing_mount")
}

/// Read a local translation while rejecting transforms this simple alignment
/// policy cannot interpret. A missing translate opinion is USD identity.
fn local_translation(doc, path) {
    let record = query("QueryUsdPrim", #{
        doc_id: doc,
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
