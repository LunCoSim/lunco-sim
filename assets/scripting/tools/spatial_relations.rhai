// Small, Rhai-authored assembly relation vocabulary over LunCoSim's native
// f64 Vec3/Quat/Transform values. These functions implement deterministic
// direct-placement relations for anchored geometry; they are not a general
// constraint-graph solver. Coupled or dynamic cases belong in the existing
// SysML-to-Modelica Rhai policy.

fn _unit_scale(scale) {
    type_of(scale) == "Vec3"
        && (scale.x - 1.0).abs() <= 1.0e-12
        && (scale.y - 1.0).abs() <= 1.0e-12
        && (scale.z - 1.0).abs() <= 1.0e-12
}

fn _unit_rotation(rotation) {
    if type_of(rotation) != "Quat" || !quat_is_valid(rotation) { return false; }
    let length_squared = rotation.x * rotation.x + rotation.y * rotation.y
        + rotation.z * rotation.z + rotation.w * rotation.w;
    (length_squared - 1.0).abs() <= 1.0e-10
}

fn _rigid_transform(value) {
    type_of(value) == "Transform"
        && transform_is_finite(value)
        && _unit_rotation(value.rotation)
        && _unit_scale(value.scale)
}

// Align a moving component-local datum with a target world datum (six DOF).
fn coincident(target_world_datum, moving_local_datum) {
    if !_rigid_transform(target_world_datum)
        || !_rigid_transform(moving_local_datum) {
        return ();
    }
    let root_rotation = target_world_datum.rotation
        * quat_inverse(moving_local_datum.rotation);
    let root_translation = target_world_datum.translation
        - qrot(root_rotation, moving_local_datum.translation);
    transform(root_translation, root_rotation, vec3(1.0, 1.0, 1.0))
}

// Align a moving datum at a typed offset from a fixed component datum. The
// offset is expressed in the fixed datum's local frame.
fn offset(fixed_component_pose, fixed_local_datum,
        moving_local_datum, offset_in_fixed_frame) {
    if !_rigid_transform(fixed_component_pose)
        || !_rigid_transform(fixed_local_datum)
        || !_rigid_transform(moving_local_datum)
        || type_of(offset_in_fixed_frame) != "Vec3"
        || !vec3_is_finite(offset_in_fixed_frame) {
        return ();
    }
    let fixed_world_datum = transform_compose(fixed_component_pose,
        fixed_local_datum);
    if type_of(fixed_world_datum) != "Transform" { return (); }
    let offset_frame = transform(offset_in_fixed_frame,
        quat_identity(), vec3(1.0, 1.0, 1.0));
    let target_world_datum = transform_compose(fixed_world_datum,
        offset_frame);
    if type_of(target_world_datum) != "Transform" { return (); }
    coincident(target_world_datum, moving_local_datum)
}

// Close the translational part of a mate while preserving the moving
// component's current rotation and scale (an anchored three-DOF relation).
fn coincident_translation(fixed_component_pose, fixed_local_datum,
        moving_component_pose, moving_local_datum) {
    if type_of(fixed_component_pose) != "Transform"
        || type_of(fixed_local_datum) != "Transform"
        || type_of(moving_component_pose) != "Transform"
        || type_of(moving_local_datum) != "Transform"
        || !transform_is_finite(fixed_component_pose)
        || !transform_is_finite(fixed_local_datum)
        || !transform_is_finite(moving_component_pose)
        || !transform_is_finite(moving_local_datum)
        || !_unit_rotation(fixed_component_pose.rotation)
        || !_unit_rotation(fixed_local_datum.rotation)
        || !_unit_rotation(moving_component_pose.rotation)
        || !_unit_rotation(moving_local_datum.rotation) {
        return ();
    }
    let fixed_world_datum = transform_compose(fixed_component_pose,
        fixed_local_datum);
    let moving_world_datum = transform_compose(moving_component_pose,
        moving_local_datum);
    if type_of(fixed_world_datum) != "Transform"
        || type_of(moving_world_datum) != "Transform" {
        return ();
    }
    let translation = moving_component_pose.translation
        + fixed_world_datum.translation - moving_world_datum.translation;
    transform(translation, moving_component_pose.rotation,
        moving_component_pose.scale)
}

// Derive a station along a component-local axis from a native datum frame and
// signed metric distance. This is useful for dimensioned struts and rails.
fn point_on_axis(frame, local_axis, signed_distance) {
    if !_rigid_transform(frame)
        || type_of(local_axis) != "Vec3" || !vec3_is_finite(local_axis)
        || !signed_distance.is_finite() || vlen(local_axis) <= 1.0e-12 {
        return ();
    }
    let local_point = vscale(vnorm(local_axis), signed_distance);
    transform_apply_point(frame, local_point)
}

fn translation_residual_m(expected, actual) {
    if type_of(expected) != "Transform" || type_of(actual) != "Transform"
        || !transform_is_finite(expected) || !transform_is_finite(actual) {
        return ();
    }
    vlen(vsub(actual.translation, expected.translation))
}

fn rotation_residual_rad(expected, actual) {
    if type_of(expected) != "Transform" || type_of(actual) != "Transform"
        || !transform_is_finite(expected) || !transform_is_finite(actual)
        || !_unit_rotation(expected.rotation) || !_unit_rotation(actual.rotation) {
        return ();
    }
    let relative = quat_inverse(expected.rotation) * actual.rotation;
    let scalar = relative.w.abs();
    2.0 * acos(if scalar > 1.0 { 1.0 } else { scalar })
}
