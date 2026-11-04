use avian3d::prelude::JointDamping;
use bevy::math::{DMat4, DQuat, DVec3};
use lunco_usd_avian_contracts::{JointDrive, PendingUsdJoint};
use lunco_usd_bevy_stage::world_transform_matrix_d_at;
use openusd::sdf::Path as SdfPath;
use openusd_schemas::physics::{DriveType, tokens as ptok};

use crate::{read_authored_quat, read_authored_real, read_authored_vec3};

/// Read a standard PhysicsDriveAPI law and normalize targets, gains and caps
/// into metres/radians, seconds and physical force/torque. Shared by ordinary
/// joints and multi-body gear relations.
pub fn read_drive(
    view: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
    ns: &str,
) -> Result<Option<JointDrive>, ()> {
    let api_name = format!("PhysicsDriveAPI:{ns}");
    if !view.has_api_schema(path, &api_name) {
        return Ok(None);
    }
    // Drive quantities convert by their SPEC units, per instance. An angular
    // drive authors DEGREES (`targetPosition` deg, `targetVelocity` deg/s,
    // stiffness/damping per degree) and its torques carry distance² — so
    // targets go deg→rad and gains ×(180/π). Only force-based gains carry
    // metersPerUnit²; acceleration-based gains are mass-normalized.
    // Torque caps always carry metersPerUnit².
    // A linear drive authors stage units: targets and `maxForce` (mass ·
    // distance / s²) scale by metersPerUnit; its stiffness (mass/s²) and
    // damping (mass/s) are distance-free and pass through.
    let conv = lunco_usd_bevy_stage::stage_convention(view).map_err(|_| ())?;
    let property =
        |template: &str| openusd::usd::SchemaRegistry::make_name_instance(template, ns).to_string();
    let type_name = property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_TYPE);
    let ty = match view.attr_value(path, &type_name) {
        Some(value) => Some(value.get::<DriveType>().ok_or(())?),
        None if !view.has_authored_attribute(path, &type_name) => None,
        None => return Err(()),
    };
    let acceleration = ty == Some(DriveType::Acceleration);
    let angular = ns == "angular";
    let target = |v: f64| {
        if angular {
            v.to_radians()
        } else {
            conv.length(v)
        }
    };
    let gain = |v: f64| {
        if angular {
            if acceleration {
                v.to_degrees()
            } else {
                conv.length(conv.length(v.to_degrees()))
            }
        } else {
            v
        }
    };
    let force = |v: f64| {
        if angular {
            conv.length(conv.length(v))
        } else {
            conv.length(v)
        }
    };
    let read_value =
        |name: &str| -> Result<Option<f64>, ()> { read_authored_real(view, path, name) };
    let target_position_name =
        property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_TARGET_POSITION);
    let target_velocity_name =
        property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_TARGET_VELOCITY);
    let max_force_name = property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_MAX_FORCE);
    let stiffness_name = property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_STIFFNESS);
    let damping_name = property(ptok::DRIVE_MULTIPLE_APPLY_TEMPLATE_PHYSICS_DAMPING);
    let tp = read_value(&target_position_name)?.map(target);
    let tv = read_value(&target_velocity_name)?.map(target);
    let mf = read_value(&max_force_name)?.map(force);
    let k = read_value(&stiffness_name)?.map(gain);
    let c = read_value(&damping_name)?.map(gain);
    Ok(
        (tp.is_some() || tv.is_some() || mf.is_some() || k.is_some() || c.is_some()).then_some(
            JointDrive {
                target_position: tp,
                target_velocity: tv,
                max_force: mf,
                stiffness: k,
                damping: c,
                drive_type: ty,
            },
        ),
    )
}

/// True when some ancestor prim of `sdf_path` is a rigid body — i.e. this prim's
/// collider is a piece of that body's compound shape rather than a body (or
/// standalone static collider) in its own right.
///
/// One spelling of "this is a body": an applied `PhysicsRigidBodyAPI`. Nothing else
/// makes a prim a body.
///
/// Walks the composed prim hierarchy through the shared reader boundary, so it
/// answers the same way for the prepared initial plan and the live edited stage,
/// independently of where the prim happens to sit in the ECS.
pub fn has_rigid_body_ancestor(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> bool {
    let mut cur = sdf_path.parent();
    while let Some(p) = cur {
        if p.is_abs_root() {
            return false;
        }
        if reader.has_api_schema(&p, ptok::PHYSICS_RIGID_BODY_API) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Does this prim BECOME a body in avian? Mirrors the two arms of
/// [`process_usd_avian_prims`] that insert a `RigidBody`, and must keep mirroring
/// them: `PhysicsRigidBodyAPI` (dynamic/kinematic/static per its own attributes),
/// terrain (always static), or a collider with no rigid-body ancestor, which the
/// USD physics spec makes standalone static geometry.
///
/// A collider that DOES have a rigid-body ancestor is not a body — it is folded
/// into that ancestor's compound shape — so it is deliberately not one here.
fn is_avian_body(reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject, path: &SdfPath) -> bool {
    reader.has_api_schema(path, ptok::PHYSICS_RIGID_BODY_API)
        || reader.has_api_schema(path, "LunCoTerrainAPI")
        || (reader.has_api_schema(path, ptok::PHYSICS_COLLISION_API)
            && !has_rigid_body_ancestor(reader, path))
}

/// Resolve the relationship target's native rigid owner through composed USD.
/// A collider without a rigid ancestor is a standalone static body. A valid
/// target without any body ancestor instead defines a frame fixed in world;
/// the endpoint reader retains that target's transform for its world anchor.
pub fn nearest_body_path(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Option<SdfPath> {
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            return None;
        }
        if is_avian_body(reader, &p) {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

/// Resolve a USD joint relationship target to the rigid-body prim that owns
/// the endpoint.
pub fn resolve_joint_body_path(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    target: &str,
) -> Option<String> {
    let path = SdfPath::new(target).ok()?;
    nearest_body_path(reader, &path).map(|resolved| resolved.to_string())
}
/// Convert a joint endpoint into the scale-free native body's frame. USD
/// anchors are local to the relationship target; an empty target is world.
/// Matrix composition stays f64, including cancellation of world translations.
fn endpoint_frame(
    body_world: DMat4,
    target_world: DMat4,
    position: DVec3,
    rotation: DQuat,
    target_is_body: bool,
) -> Option<(DVec3, DQuat)> {
    let affine = |matrix: DMat4| {
        matrix.is_finite()
            && matrix.row(3).abs_diff_eq(bevy::math::DVec4::W, 1e-12)
            && matrix.determinant().is_finite()
            && matrix.determinant() != 0.0
    };
    if !affine(body_world) || !affine(target_world) {
        return None;
    }
    // Gf stores row-vector matrices; Glam's column storage is its transpose.
    // Factor with zero threshold after the nonsingular check so valid tiny
    // scales are preserved rather than replaced by a factorization tolerance.
    let body = openusd::gf::Mat4d(body_world.to_cols_array()).factor(0.0);
    let scale = DVec3::new(body.scale.x, body.scale.y, body.scale.z);
    if !scale.is_finite() || !scale.cmpne(DVec3::ZERO).all() {
        return None;
    }
    if target_is_body {
        return Some((position * scale, rotation));
    }
    let local =
        body_world.inverse() * target_world * DMat4::from_rotation_translation(rotation, position);
    if !affine(local) {
        return None;
    }
    let local = openusd::gf::Mat4d(local.to_cols_array()).factor(0.0);
    let mut rigid_rotation = local.rotation;
    if !rigid_rotation.orthonormalize() {
        return None;
    }
    let q = rigid_rotation.extract_rotation_quat();
    let rotation = DQuat::from_xyzw(q.x, q.y, q.z, q.w);
    if !rotation.is_finite() || rotation.length_squared() <= f64::EPSILON {
        return None;
    }
    let rotation = rotation.normalize();
    let position = DVec3::new(
        local.translation.x,
        local.translation.y,
        local.translation.z,
    ) * scale;
    (position.is_finite() && rotation.is_finite() && rotation.is_normalized())
        .then_some((position, rotation))
}

fn read_joint_endpoint(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    joint: &SdfPath,
    target: Option<&SdfPath>,
    position_attr: &str,
    rotation_attr: &str,
) -> Option<(String, DVec3, DQuat)> {
    let convention = lunco_usd_bevy_stage::stage_convention(reader).ok()?;
    let position = convention.point_d(
        read_authored_vec3(reader, joint, position_attr)
            .ok()?
            .unwrap_or(DVec3::ZERO),
    );
    let rotation = convention.rotation_d(
        read_authored_quat(reader, joint, rotation_attr)
            .ok()?
            .unwrap_or(DQuat::IDENTITY),
    );
    let Some(target) = target else {
        return Some((String::new(), position, rotation));
    };
    if !reader.has_prim(target) {
        return None;
    }
    let body = nearest_body_path(reader, target);
    let body_world = match &body {
        Some(body) => world_transform_matrix_d_at(reader, body, 0.0).ok()?,
        None => DMat4::IDENTITY,
    };
    let target_world = world_transform_matrix_d_at(reader, target, 0.0).ok()?;
    let (position, rotation) = endpoint_frame(
        body_world,
        target_world,
        position,
        rotation,
        body.as_ref() == Some(target),
    )?;
    Some((
        body.map(|body| body.to_string()).unwrap_or_default(),
        position,
        rotation,
    ))
}

/// The USD joint base fields shared by every concrete joint reader.
struct JointBaseRead {
    body0: String,
    body1: String,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
}

/// Read the STANDARD UsdPhysics joint at `path` through the shared composed
/// reader into the deferred [`PendingUsdJoint`]. The reader is either the
/// worker-produced initial projection plan or the live reader for an authored
/// generation; the joint contract is identical in both cases.
///
/// This reads the USD standard concrete joint type, shared body/frame
/// relationships, `UsdPhysicsDriveAPI`, and per-DOF `UsdPhysicsLimitAPI` for
/// the generic-D6 reduction. Returns `None` when
/// `path` is not a UsdPhysics joint or has an invalid body reference.
/// Revolute limits are converted degrees→radians
/// (the `PendingUsdJoint` contract); prismatic/distance limits convert to metres.
pub fn read_joint_spec(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Option<PendingUsdJoint> {
    read_joint_spec_with_policy(reader, path, true)
}

/// Read a joint for the authored physics linter, including an explicitly
/// `lunco:lintOnly` fixture.  Such a fixture is still part of the composed
/// authoring that the linter must inspect, but it is not a runtime joint.  The
/// distinction is intentional: a malformed test asset must not become a live
/// constraint merely because the test needs to prove that the linter catches
/// it.
pub fn read_joint_spec_for_lint(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Option<PendingUsdJoint> {
    read_joint_spec_with_policy(reader, path, false)
}

/// Whether a standard joint is executable rather than disabled or lint-only.
/// Projection and topology admission consume the same participation contract.
pub fn is_runtime_joint_enabled(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> bool {
    reader.boolean(path, "lunco:lintOnly") != Some(true)
        && reader.boolean(path, ptok::PHYSICS_JOINT_ENABLED) != Some(false)
}

fn read_joint_spec_with_policy(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
    skip_lint_only: bool,
) -> Option<PendingUsdJoint> {
    let view = reader;
    // `lintOnly` is a narrow authoring contract for deliberately malformed
    // fixtures. It is checked only by the runtime reader; the linter calls the
    // companion entry point above so it still sees and evaluates the fixture.
    if skip_lint_only && !is_runtime_joint_enabled(reader, path) {
        return None;
    }
    // `physics:jointEnabled` (schema default true) is the spec's own way to say a
    // joint is not simulated. It matters now that COMPONENTS own their mount
    // joints: a host that wants the mechanism inert — `ground_station.usda` parks
    // its dish and disables both link bodies, because a station with no target to
    // track is better still than swinging — needs a way to say so without editing
    // the component. `over "YawJoint" { uniform bool physics:jointEnabled = false }`
    // is that way, and it is stock UsdPhysics rather than anything invented here.
    let joint_enabled = match view.boolean(path, ptok::PHYSICS_JOINT_ENABLED) {
        Some(value) => value,
        None if !view.has_authored_attribute(path, ptok::PHYSICS_JOINT_ENABLED) => true,
        None => return None,
    };
    if !joint_enabled {
        return None;
    }
    // **Units/axes convert here** (doc 41). `axis` names an axis of the STAGE's
    // frame, so on a Z-up stage an authored `"Z"` is *up* — canonical up is +Y.
    // Read raw it would hinge about the wrong axis while the meshes and colliders
    // (which do convert, via `local_transform_at`) sit correctly: a silently
    // wrong joint in a visually right assembly.
    let conv = lunco_usd_bevy_stage::stage_convention(view).ok()?;
    let read_real_or_default = |name: &str, default: f64| -> Option<f64> {
        match view.real(path, name) {
            Some(value) => Some(value),
            None if !view.has_authored_attribute(path, name) => Some(default),
            None => None,
        }
    };
    let damping = if view.has_api_schema(path, "LunCoJointDampingAPI") {
        let linear = read_real_or_default("lunco:jointDamping:linear", 0.0)?;
        let angular = read_real_or_default("lunco:jointDamping:angular", 0.0)?;
        if ![linear, angular]
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.0 && value <= f64::from(f32::MAX))
        {
            return None;
        }
        Some(JointDamping {
            linear: linear as f32,
            angular: angular as f32,
        })
    } else {
        None
    };
    // A UsdPhysics joint is defined by a FRAME on each body: `physics:localPos0`
    // + `physics:localRot0` on body0, `localPos1` + `localRot1` on body1. The
    // joint constrains those two frames to each other, and `physics:axis` names a
    // cardinal axis *of the joint frame* — X/Y/Z is the entire vocabulary the
    // schema has, which is why `localRot` exists: it is how a raked mechanism (a
    // landing leg 25° off vertical) says where its axis really points. avian
    // spells the same thing as `JointFrame { anchor, basis }`, with
    // `slider_axis`/`hinge_axis`/`twist_axis` likewise read in the basis
    // (`free_axis1 = rotation1 * basis1 * slider_axis`), so the two models map
    // one-to-one and the axis stays CARDINAL on both sides.
    //
    // Both halves of the frame must cross: every avian joint but the spherical
    // locks relative orientation through `basis1`/`basis2`, so an identity basis
    // constrains its body to the OTHER body's orientation. Carrying the rake in
    // the axis alone would aim the slider correctly and still wrench the strut
    // square to the hull.
    let axis_of = |axis: &str| -> Option<DVec3> {
        Some(conv.dir_d(match axis {
            ptok::X => DVec3::X,
            ptok::Y => DVec3::Y,
            ptok::Z => DVec3::Z,
            _ => return None,
        }))
    };
    let read_axis = || -> Option<DVec3> {
        let axis = match view.text(path, ptok::PHYSICS_AXIS) {
            Some(axis) if matches!(axis.as_str(), ptok::X | ptok::Y | ptok::Z) => axis,
            Some(_) => return None,
            None if !view.has_authored_attribute(path, ptok::PHYSICS_AXIS) => ptok::X.to_string(),
            None => return None,
        };
        axis_of(&axis)
    };
    // Each endpoint independently uses the schema's zero position and identity
    // rotation defaults, then resolves its relationship target into a body frame.
    let base = || -> Option<JointBaseRead> {
        let targets0 = reader.rel_targets(path, ptok::PHYSICS_BODY0);
        let targets1 = reader.rel_targets(path, ptok::PHYSICS_BODY1);
        if targets0.len() > 1 || targets1.len() > 1 {
            return None;
        }
        let (b0, lp0, local_rot0) = read_joint_endpoint(
            reader,
            path,
            targets0.first(),
            ptok::PHYSICS_LOCAL_POS0,
            ptok::PHYSICS_LOCAL_ROT0,
        )?;
        let (b1, lp1, local_rot1) = read_joint_endpoint(
            reader,
            path,
            targets1.first(),
            ptok::PHYSICS_LOCAL_POS1,
            ptok::PHYSICS_LOCAL_ROT1,
        )?;
        if b0 == b1 {
            return None;
        }
        Some(JointBaseRead {
            body0: b0,
            body1: b1,
            local_pos0: lp0,
            local_pos1: lp1,
            local_rot0,
            local_rot1,
        })
    };
    let read_limit = |name: &str, default: f64| -> Option<f64> {
        match view.real(path, name) {
            Some(value) if !value.is_nan() => Some(value),
            None if !view.has_authored_attribute(path, name) => Some(default),
            _ => None,
        }
    };

    // Every arm builds the same `PendingUsdJoint` shape off the shared
    // `JointBaseRead`; only the axis, the limits and the drive differ by type.
    let pending_from = |b: JointBaseRead,
                        axis: DVec3,
                        limit_lower: f64,
                        limit_upper: f64,
                        joint_type: &str,
                        swing_limit: Option<(f64, f64)>,
                        drive: Option<JointDrive>| PendingUsdJoint {
        body0_path: b.body0,
        body1_path: b.body1,
        axis,
        local_pos0: b.local_pos0,
        local_pos1: b.local_pos1,
        local_rot0: b.local_rot0,
        local_rot1: b.local_rot1,
        limit_lower,
        limit_upper,
        joint_type: joint_type.into(),
        swing_limit,
        drive,
        damping,
    };

    let type_name = view.type_name(path)?;
    let spec = match type_name.as_str() {
        ptok::PHYSICS_REVOLUTE_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            let lo = read_limit(ptok::PHYSICS_LOWER_LIMIT, f64::NEG_INFINITY)?.to_radians();
            let hi = read_limit(ptok::PHYSICS_UPPER_LIMIT, f64::INFINITY)?.to_radians();
            let drive = read_drive(view, path, ptok::ANGULAR).ok()?;
            pending_from(b, axis, lo, hi, ptok::PHYSICS_REVOLUTE_JOINT, None, drive)
        }
        ptok::PHYSICS_PRISMATIC_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            // Linear limits are authored in scene units, like the anchors.
            let lo = conv.length(read_limit(ptok::PHYSICS_LOWER_LIMIT, f64::NEG_INFINITY)?);
            let hi = conv.length(read_limit(ptok::PHYSICS_UPPER_LIMIT, f64::INFINITY)?);
            let drive = read_drive(view, path, ptok::LINEAR).ok()?;
            pending_from(b, axis, lo, hi, ptok::PHYSICS_PRISMATIC_JOINT, None, drive)
        }
        ptok::PHYSICS_SPHERICAL_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            // `physics:coneAngle{0,1}Limit` is in degrees, while Avian's
            // `AngleLimit` is in radians.
            let cone0 = read_limit(ptok::PHYSICS_CONE_ANGLE0_LIMIT, -1.0)?;
            let cone1 = read_limit(ptok::PHYSICS_CONE_ANGLE1_LIMIT, -1.0)?;
            let swing = (cone0 >= 0.0 || cone1 >= 0.0)
                .then_some((cone0.max(0.0).to_radians(), cone1.max(0.0).to_radians()));
            pending_from(
                b,
                axis,
                f64::NEG_INFINITY,
                f64::INFINITY,
                ptok::PHYSICS_SPHERICAL_JOINT,
                swing,
                None,
            )
        }
        ptok::PHYSICS_FIXED_JOINT => {
            let b = base()?;
            pending_from(
                b,
                DVec3::Y,
                f64::NEG_INFINITY,
                f64::INFINITY,
                ptok::PHYSICS_FIXED_JOINT,
                None,
                None,
            )
        }
        ptok::PHYSICS_DISTANCE_JOINT => {
            let b = base()?;
            // Distances are scene units; a negative authored value is the
            // schema's "limit disabled" sentinel.
            let lo = conv.length(read_limit(ptok::PHYSICS_MIN_DISTANCE, -1.0)?);
            let hi = conv.length(read_limit(ptok::PHYSICS_MAX_DISTANCE, -1.0)?);
            pending_from(
                b,
                DVec3::Y,
                lo,
                hi,
                ptok::PHYSICS_DISTANCE_JOINT,
                None,
                None,
            )
        }
        ptok::PHYSICS_JOINT => {
            // Generic/D6 reduces through per-DOF UsdPhysicsLimitAPI.
            let b = base()?;
            let (reduced, cardinal, lo, hi, is_rot) = reduce_generic_joint(reader, path)?;
            // Same two conversions every typed arm applies: the cardinal axis is named
            // in the STAGE's frame, and an angular limit is authored in degrees while a
            // linear one is in scene units. `to_radians`/`length` leave an infinite
            // (unauthored) bound infinite.
            let axis = conv.dir_d(cardinal);
            let (lo, hi) = if is_rot {
                (lo.to_radians(), hi.to_radians())
            } else {
                (conv.length(lo), conv.length(hi))
            };
            pending_from(b, axis, lo, hi, reduced, None, None)
        }
        _ => return None,
    };

    if ![spec.local_pos0, spec.local_pos1, spec.axis]
        .into_iter()
        .all(lunco_physics::avian_backend_vector_is_valid)
        || ![spec.local_rot0, spec.local_rot1]
            .into_iter()
            .all(lunco_physics::avian_backend_rotation_is_valid)
        || [spec.limit_lower, spec.limit_upper]
            .into_iter()
            .any(|value| value.is_nan() || (value.is_finite() && value.abs() > f64::from(f32::MAX)))
        || spec.swing_limit.is_some_and(|(a, b)| {
            [a, b]
                .into_iter()
                .any(|value| !value.is_finite() || value.abs() > f64::from(f32::MAX))
        })
    {
        return None;
    }

    Some(spec)
}

/// Reduce a generic `UsdPhysicsJoint` (D6) to the Avian primitive matching its
/// free degrees of freedom by reading each per-DOF `UsdPhysicsLimitAPI`
/// (`limit:{transX..rotZ}`). A DOF is locked when `low > high` and free when
/// the limit schema is absent or its bounds are unauthored.
fn reduce_generic_joint(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Option<(&'static str, DVec3, f64, f64, bool)> {
    const DOFS: [(&str, DVec3, bool); 6] = [
        (ptok::TRANS_X, DVec3::X, false),
        (ptok::TRANS_Y, DVec3::Y, false),
        (ptok::TRANS_Z, DVec3::Z, false),
        (ptok::ROT_X, DVec3::X, true),
        (ptok::ROT_Y, DVec3::Y, true),
        (ptok::ROT_Z, DVec3::Z, true),
    ];
    let mut free_trans: Vec<(DVec3, f64, f64)> = Vec::new();
    let mut free_rot: Vec<(DVec3, f64, f64)> = Vec::new();
    for (inst, axis, is_rot) in DOFS {
        let api_name = format!("{}:{inst}", ptok::PHYSICS_LIMIT_API);
        let property = |template: &str| {
            openusd::usd::SchemaRegistry::make_name_instance(template, inst).to_string()
        };
        let read_bound = |name: &str, default: f64| match reader.real(path, name) {
            Some(value) if !value.is_nan() => Some(value),
            None if !reader.has_authored_attribute(path, name) => Some(default),
            _ => None,
        };
        let low_name = property(ptok::LIMIT_MULTIPLE_APPLY_TEMPLATE_PHYSICS_LOW);
        let high_name = property(ptok::LIMIT_MULTIPLE_APPLY_TEMPLATE_PHYSICS_HIGH);
        let (low, high) = match reader.has_api_schema(path, &api_name) {
            true => (
                read_bound(&low_name, f64::NEG_INFINITY)?,
                read_bound(&high_name, f64::INFINITY)?,
            ),
            false => (f64::NEG_INFINITY, f64::INFINITY),
        };
        match (low, high) {
            (l, h) if l > h => {} // locked
            (l, h) => {
                let entry = (axis, l, h);
                if is_rot {
                    free_rot.push(entry)
                } else {
                    free_trans.push(entry)
                }
            }
        }
    }
    // The axis is returned CARDINAL and the limits in their AUTHORED units; the
    // caller converts both, exactly as it does for a natively-typed joint. The
    // trailing flag says whether the surviving DOF is rotational, which is what
    // decides whether the limits are degrees (`limit:rot*`) or metres
    // (`limit:trans*`).
    match (free_trans.len(), free_rot.len()) {
        (0, 0) => Some((
            "PhysicsFixedJoint",
            DVec3::Y,
            f64::NEG_INFINITY,
            f64::INFINITY,
            false,
        )),
        (0, 1) => Some((
            "PhysicsRevoluteJoint",
            free_rot[0].0,
            free_rot[0].1,
            free_rot[0].2,
            true,
        )),
        (1, 0) => Some((
            "PhysicsPrismaticJoint",
            free_trans[0].0,
            free_trans[0].1,
            free_trans[0].2,
            false,
        )),
        (0, 3) => Some((
            "PhysicsSphericalJoint",
            free_rot[0].0,
            f64::NEG_INFINITY,
            f64::INFINITY,
            true,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod endpoint_frame_tests {
    use super::endpoint_frame;
    use bevy::math::{DMat4, DQuat, DVec3};

    #[test]
    fn endpoint_lowering_preserves_scale_target_frame_and_large_world_precision() {
        let rotation = DQuat::from_rotation_y(0.7);
        let anchor_rotation = DQuat::from_rotation_x(0.4);
        for scale in [2.0, 1e-13] {
            let body = DMat4::from_scale_rotation_translation(
                DVec3::splat(scale),
                rotation,
                DVec3::new(1e8, 2.0, 0.0),
            );
            let (position, orientation) =
                endpoint_frame(body, body, DVec3::X, anchor_rotation, true).unwrap();
            assert!(position.abs_diff_eq(DVec3::X * scale, scale * 1e-12));
            assert!(orientation.abs_diff_eq(anchor_rotation, 1e-12));
        }
        let body = DMat4::from_scale_rotation_translation(
            DVec3::splat(2.0),
            DQuat::IDENTITY,
            DVec3::new(1e8, 2.0, 0.0),
        );
        let target = body * DMat4::from_rotation_translation(rotation, DVec3::X);
        let (position, orientation) =
            endpoint_frame(body, target, DVec3::X, anchor_rotation, false).unwrap();
        assert!(position.abs_diff_eq((DVec3::X + rotation * DVec3::X) * 2.0, 1e-8));
        assert!(orientation.abs_diff_eq(rotation * anchor_rotation, 1e-12));
        let (position, _) =
            endpoint_frame(body, target, DVec3::ZERO, DQuat::IDENTITY, false).unwrap();
        assert_eq!(position, DVec3::X * 2.0);
        assert!(
            endpoint_frame(
                DMat4::from_scale(DVec3::ZERO),
                target,
                DVec3::ZERO,
                DQuat::IDENTITY,
                false,
            )
            .is_none()
        );
    }
}
