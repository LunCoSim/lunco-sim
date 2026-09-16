use avian3d::physics_transform::{Position, Rotation};
use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_usd_bevy_core::{Purpose, TransformReadError, effective_purpose, local_transform_at};
use lunco_usd_bevy_scene::{
    ShapeDims, read_primitive_axis, read_shape_dims, read_usd_mesh_indexed, usd_axis_to_quat,
};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::Path as SdfPath;

use crate::read_authored_bool_or_default;

/// Checks if a USD prim has a specific API schema applied.
/// Collects collider shapes from all descendant prims of a compound body root,
/// reading directly from the USD stage.
///
/// Returns a list of `(Position, Rotation, Collider)` tuples for `Collider::compound()`.
#[derive(Debug)]
pub enum ColliderProjectionError {
    Transform(TransformReadError),
    Backend { prim: String, detail: String },
}

impl std::fmt::Display for ColliderProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transform(error) => error.fmt(f),
            Self::Backend { prim, detail } => write!(f, "{prim}: {detail}"),
        }
    }
}

impl std::error::Error for ColliderProjectionError {}

/// Project one explicitly authored USD collision prim into an Avian collider.
///
/// This is the shared projection boundary for specialized realizations such as
/// physical vehicle wheels. It accepts only a prim carrying
/// `PhysicsCollisionAPI` and a supported USD geometry type. In particular, it
/// never derives a collider from a domain parameter such as wheel radius or
/// width. Missing or unsupported authored geometry is an error at the owner.
pub fn authored_collider_from_usd(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<Collider, ColliderProjectionError> {
    if !reader.has_api_schema(sdf_path, ptok::API_COLLISION) {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "missing PhysicsCollisionAPI on authored collision geometry".to_owned(),
        });
    }

    let collision_enabled =
        match read_authored_bool_or_default(reader, sdf_path, ptok::A_COLLISION_ENABLED, true) {
            Ok(value) => value,
            Err(()) => {
                return Err(ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: format!("malformed {}", ptok::A_COLLISION_ENABLED),
                });
            }
        };
    if !collision_enabled {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: format!("{} is authored false", ptok::A_COLLISION_ENABLED),
        });
    }

    let collider = build_collider_from_usd(reader, sdf_path)?.ok_or_else(|| {
        ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "authored collision geometry has no supported shape or valid mesh data"
                .to_owned(),
        }
    })?;
    if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "collider local bounds are not finite, ordered, or f32-representable"
                .to_owned(),
        });
    }
    Ok(collider)
}

pub fn collect_child_colliders_from_usd(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    parent_path: &SdfPath,
) -> Result<Vec<(Position, Rotation, Collider)>, ColliderProjectionError> {
    let mut shapes = Vec::new();
    let convention = lunco_usd_bevy_core::stage_convention(reader).map_err(|_| {
        ColliderProjectionError::Transform(TransformReadError {
            prim: parent_path.as_str().to_owned(),
        })
    })?;

    // Per the spec a rigid body aggregates ALL descendant colliders, not only
    // direct children — a collider under an intermediate grouping `Xform`
    // (`/Rover/Shapes/Hull`) is still this body's geometry. Descendant walk with
    // each prim's transform composed in the root's frame; recursion stops at a
    // nested-body boundary (see `gather_compound_candidates`).
    let mut candidates = Vec::new();
    gather_compound_candidates(reader, parent_path, Transform::IDENTITY, &mut candidates)
        .map_err(ColliderProjectionError::Transform)?;
    // `PhysicsCollisionAPI` is valid on the rigid-body prim itself. When that
    // body also has descendants, its own shape must become the identity member
    // of the compound; otherwise the presence of any child silently discards
    // part of the authored collision contract. Its local transform is already
    // the ECS body's transform, so the shape is untransformed in body space.
    if !candidates.is_empty() && reader.has_api_schema(parent_path, ptok::API_COLLISION) {
        candidates.insert(0, (parent_path.clone(), Transform::IDENTITY));
    }

    // `UsdGeomImageable.purpose` decides which of a body's descendants are its
    // COLLISION geometry, when the body carries more than one description of its
    // own shape. That is the standard way to say "this cheap box is what you
    // collide, that mesh is what you look at", and it is why a proxy exists at
    // all: `proxy` wins over `render` for physics, exactly as `render` wins over
    // `proxy` for drawing.
    let has_proxy = candidates
        .iter()
        .any(|(c, _)| effective_purpose(reader, c) == Purpose::Proxy);

    for (child_path, mut child_tf) in candidates {
        // A typed trigger zone owns an overlap sensor entity of its own.  It is
        // deliberately not part of an ancestor body's compound collider: doing
        // both would create a second, solid copy attached to the rigid body and
        // the vehicle would be pushed by the touchdown volume before contact.
        // `lunco:triggerZone` is the authored semantic contract; this is not a
        // path/name exception.
        if reader
            .text(&child_path, "lunco:triggerZone")
            .is_some_and(|zone| !zone.trim().is_empty())
        {
            continue;
        }
        // `guide` is annotation — a debug axis, a sensor cone, a planned path. It
        // is never physical, whatever geometry it happens to be made of.
        let purpose = effective_purpose(reader, &child_path);
        if purpose == Purpose::Guide {
            continue;
        }
        // With a proxy present, the render geometry is NOT also a collider —
        // folding both in would collide the vehicle twice, once at each level of
        // detail, and the expensive one would win every contact.
        if has_proxy && purpose == Purpose::Render {
            continue;
        }

        // Only descendants that APPLY `PhysicsCollisionAPI` are colliders — same
        // rule the standalone arm uses. Bare geometry (a light housing, a decal
        // plane) contributes nothing to the compound shape.
        if !reader.has_api_schema(&child_path, ptok::API_COLLISION) {
            continue;
        }

        // The standard schema default is enabled, but an authored value of the
        // wrong type is malformed data, not an omitted default. Do not let a
        // bad collision flag silently turn a visual/physics mismatch into a
        // solid collider.
        let child_collision = match read_authored_bool_or_default(
            reader,
            &child_path,
            ptok::A_COLLISION_ENABLED,
            true,
        ) {
            Ok(value) => value,
            Err(()) => {
                error!(
                    "[usd-avian] {child_path} has malformed {}; refusing collider projection",
                    ptok::A_COLLISION_ENABLED
                );
                continue;
            }
        };
        if !child_collision {
            continue;
        }

        // For Cylinder children, fold UsdGeomCylinder.axis into the
        // child's compound-local rotation so the Y-axis collider lines
        // up with the authored axis (mirrors what lunco-usd-bevy does
        // for the entity Transform — same canonical `usd_axis_to_quat`).
        // The body root is different: its axis is already on the ECS body
        // transform, so applying it again inside the compound would rotate the
        // root shape twice.
        let is_body_shape = child_path.as_str() == parent_path.as_str();
        if !is_body_shape {
            if let Some(ty) = reader.type_name(&child_path) {
                if matches!(ty.as_str(), "Cylinder" | "Cone" | "Capsule" | "Plane") {
                    let Some(axis_tok) = read_primitive_axis(reader, &child_path, &ty) else {
                        continue;
                    };
                    // Pre-rotate by the stage convention: the `axis` token names an
                    // axis of the STAGE's frame while the collider is built in the
                    // canonical one (identical to what usd-bevy does for the visual
                    // Transform, so mesh and collider can't disagree on a Z-up stage).
                    let q_axis =
                        convention.orient(usd_axis_to_quat(&axis_tok).unwrap_or(Quat::IDENTITY));
                    if !q_axis.abs_diff_eq(Quat::IDENTITY, 1e-6) {
                        child_tf.rotation *= q_axis;
                    }
                }
            }
        }

        // Build collider from the child's geometry. The candidate transform
        // carries the complete scale from the body boundary to this prim;
        // unlike a standalone collider, a compound child has no ECS entity on
        // which Avian could propagate intermediate Xform scales.
        let scale = if is_body_shape {
            Vec3::ONE
        } else {
            child_tf.scale
        };
        if !lunco_physics::avian_backend_vector_is_valid(scale.as_dvec3()) {
            return Err(ColliderProjectionError::Backend {
                prim: child_path.to_string(),
                detail: "collider scale is not finite or f32-representable".to_owned(),
            });
        }
        if let Some(collider) = build_collider_from_usd_at_scale(reader, &child_path, scale) {
            let pos = Position(DVec3::new(
                child_tf.translation.x as f64,
                child_tf.translation.y as f64,
                child_tf.translation.z as f64,
            ));
            let rot = Rotation(child_tf.rotation.as_dquat());
            if !lunco_physics::avian_backend_pose_is_valid(pos.0, rot.0)
                || !lunco_physics::avian_backend_collider_shape_is_valid(&collider)
            {
                return Err(ColliderProjectionError::Backend {
                    prim: child_path.to_string(),
                    detail: "collider pose or local bounds are not finite, ordered, or f32-representable"
                        .to_owned(),
                });
            }
            if !lunco_physics::avian_backend_collider_is_leaf(&collider) {
                return Err(ColliderProjectionError::Backend {
                    prim: child_path.to_string(),
                    detail: format!(
                        "collider child has composite runtime shape {}; Avian compound children must be leaf shapes",
                        lunco_physics::avian_backend_collider_shape_kind(&collider)
                    ),
                });
            }
            shapes.push((pos, rot, collider));
        }
    }

    Ok(shapes)
}

/// Walks every descendant of a compound body root, composing each prim's local
/// transform into the root's frame.
///
/// Recursion stops at two boundaries:
/// - A descendant that is its OWN rigid body is not a piece of this body's
///   compound shape. It is a separate body, and if it is attached at all a
///   joint says so — which is how a foot mounts on a leg and a wheel on a
///   chassis. Folding its collider in as well gives one piece of geometry two
///   owners: the compound holds it rigidly in the parent's frame while the
///   joint tries to move it, and the two fight until a body leaves the world.
///   This is the same rule the loader already applies in the other direction
///   (a collider with no rigid-body ancestor is static geometry, never a
///   body): ownership stops at a body boundary, in both directions.
/// - A wheel (`physxVehicleWheel:radius`) is independent dynamics handled by
///   `lunco-usd-sim` (raycast probe or physical wheel rigid body), NOT a
///   collider piece of the chassis compound — matches the same skip in
///   `process_usd_avian_prims`.
fn gather_compound_candidates(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    path: &SdfPath,
    acc: Transform,
    out: &mut Vec<(SdfPath, Transform)>,
) -> Result<(), TransformReadError> {
    for child in reader.children(path) {
        if reader.has_api_schema(&child, ptok::API_RIGID_BODY) {
            continue;
        }
        if reader
            .real_f32(&child, "physxVehicleWheel:radius")
            .is_some()
        {
            continue;
        }
        // Local transform in the canonical decoder shared with usd-bevy, folded
        // into the accumulated root-relative frame.
        let local = local_transform_at(reader, &child, 0.0)?.unwrap_or(Transform::IDENTITY);
        let tf = acc.mul_transform(local);
        out.push((child.clone(), tf));
        gather_compound_candidates(reader, &child, tf, out)?;
    }
    Ok(())
}

/// Builds a Collider from a USD prim's geometry type and dimensions.
///
/// Builds an Avian collider from a USD shape prim.
///
/// **Scaling is NOT baked into the intrinsic shape here — Avian owns it.** `update_collider_scale`
/// sets `collider.scale = world Transform.scale` every frame for *every*
/// collider (measured: the ground collider's `scale` becomes (4000,0.2,4000)
/// from its composed transform). So each shape branch returns the **intrinsic,
/// unscaled** shape at its authored size, and the single [`apply_collider_scale`]
/// tail pre-applies the composed scale once, uniformly.
///
/// Why pre-apply at all, if Avian re-applies it anyway: Avian's pass is
/// DEFERRED, so for the first frames an un-pre-scaled collider is its tiny
/// intrinsic size and rovers fall straight through terrain (the fast-fall /
/// "crazy" on commit c6246202). Pre-setting it to the value Avian will
/// compute makes the collider correct from frame 0; Avian's
/// `scale != collider.scale()` guard then skips the redundant pass — no
/// double-scale, no startup race. Baking `size*scale` into the shape instead
/// (the original bug) double-scales it (`size*scale × scale`) → oversized
/// terrain → rovers float.
///
/// Spec-compliant shape attributes (UsdGeomCube/Sphere/Cylinder):
/// - **Cube**: `double size` (default 2.0).
/// - **Sphere**: `double radius` (default 1.0).
/// - **Cylinder**: `double radius`, `double height` (defaults 1, 2). Avian's
///   cylinder is Y-axial; the `UsdGeomCylinder.axis` token is honoured by the
///   entity's Transform rotation (composed in `lunco-usd-bevy`; compound
///   children get the axis rotation added in `collect_child_colliders_from_usd`).
///
/// `UsdGeomCube` is cubic: `size` is its only dimension. A non-uniform box is
/// `size` plus a non-uniform `xformOp:scale`, which the scale tail applies.
pub fn build_collider_from_usd(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<Option<Collider>, ColliderProjectionError> {
    let scale = local_transform_at(reader, sdf_path, 0.0)
        .map_err(ColliderProjectionError::Transform)?
        .map_or(Vec3::ONE, |transform| transform.scale);
    if !lunco_physics::avian_backend_vector_is_valid(scale.as_dvec3()) {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "collider scale is not finite or f32-representable".to_owned(),
        });
    }
    Ok(build_collider_from_usd_at_scale(reader, sdf_path, scale))
}

/// Build a collider with a scale already composed from the owning body frame to
/// the geometry prim. Standalone colliders obtain this from their own local
/// transform; compound children obtain it from [`gather_compound_candidates`],
/// because intermediate USD Xforms have no corresponding Avian collider entity.
pub fn build_collider_from_usd_at_scale(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    sdf_path: &SdfPath,
    scale: Vec3,
) -> Option<Collider> {
    let ty = reader.type_name(sdf_path)?;

    // Native UsdGeomMesh → static triangle-mesh collider, decoded from the
    // SAME `points`/`faceVertexIndices` `lunco-usd-bevy` renders (one geometry
    // source, so collider and visual can't drift). `set_scale` on a trimesh
    // scales its vertices exactly (no convex-hull tessellation), so the shared
    // scale tail applies unchanged.
    if ty == "Mesh" {
        let (verts, tris) = read_usd_mesh_indexed(reader, sdf_path)?;
        let verts: Vec<DVec3> = verts
            .into_iter()
            .map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64))
            .collect();
        // Standard `UsdPhysicsMeshCollisionAPI physics:approximation` selects how
        // the render mesh becomes a collider. Default (unauthored / `none` /
        // `meshSimplification`) = exact triangle mesh — correct for STATIC terrain.
        // `convexHull`/`convexDecomposition` produce the solid volumes a DYNAMIC
        // body needs (a trimesh can't be a moving rigid body in parry). Read via
        // the standard token so it works through either composed reader;
        // an authored approximation that this adapter cannot realize is rejected.
        // `physics:approximation` is a property OF `PhysicsMeshCollisionAPI`, so
        // it only means anything when that schema is applied.
        let approximation = reader
            .has_api_schema(sdf_path, ptok::API_MESH_COLLISION)
            .then(|| reader.text(sdf_path, ptok::A_APPROXIMATION))
            .flatten();
        let collider = match approximation.as_deref() {
            Some("convexHull") => Collider::convex_hull(verts)?,
            Some("convexDecomposition") => Collider::convex_decomposition(verts, tris),
            None | Some("none") => Collider::try_trimesh(verts, tris).ok()?,
            // The authored approximation is a physical contract. Do not
            // silently replace an unsupported approximation with a different
            // shape, and do not turn a failed convex hull into a dynamic
            // triangle mesh that Avian cannot use as a moving body.
            Some(_) => return None,
        };
        return Some(apply_collider_scale(collider, scale));
    }

    // Dimensions (+ their magic defaults) come from the canonical
    // `read_shape_dims` shared with usd-bevy's mesh builder, so the
    // collider can't desync from the visual mesh. Build the INTRINSIC
    // (unscaled) shape; the scale tail below owns scaling.
    let shape_dims = read_shape_dims(reader, sdf_path, ty.as_str())?;
    let collider = match shape_dims {
        ShapeDims::Cube { size } => Collider::cuboid(size, size, size),
        ShapeDims::Sphere { radius } => Collider::sphere(radius),
        ShapeDims::Cylinder { radius, height } => Collider::cylinder(radius, height),
        ShapeDims::Cone { radius, height } => Collider::cone(radius, height),
        ShapeDims::Capsule { radius, height } => Collider::capsule(radius, height),
        // Represent the plane as a thin cuboid so bounds and scaling
        // behave predictably and match the visual mapping.
        ShapeDims::Plane { width, length } => Collider::cuboid(width, 0.001, length),
    };

    Some(apply_collider_scale(collider, scale))
}

/// Pre-applies a prim's composed USD scale to a freshly-built intrinsic collider so
/// it is correct from frame 0, matching what Avian's `update_collider_scale` will
/// compute. See [`build_collider_from_usd`] for why this is the *only* place
/// scale touches a collider.
///
/// Note Avian's scale pass is **change-driven, not per-frame**: it's gated by
/// `Or<(Changed<Transform>, Changed<C>)>` plus an inner `scale != collider.scale()`
/// guard, so for static terrain it runs once at frame 0 and never again — and
/// because our pre-apply makes that first pass a no-op, the value we set here is
/// what survives.
///
/// The `10` is the **subdivision count**: facets used when a NON-UNIFORM scale
/// forces a round collider (sphere/cylinder/cone/capsule) to be re-tessellated
/// into a convex hull. Cuboids ignore it (a box stays exact under any scale), so
/// it's a no-op for terrain and only matters for scaled round shapes. We hardcode
/// `10` to match Avian's own hardcoded value (backend.rs `update_collider_scale`,
/// which carries a literal `// TODO: Support configurable subdivision count`) —
/// matching it means our pre-applied collider has the same fidelity Avian would
/// produce, so they never disagree.
///
/// TODO(realtime subdivisions): make this authorable + live-tunable per prim once
/// Avian exposes a configurable subdivision count (its TODO above). The proper
/// shape is a USD `int physics:collider:scaleSubdivisions` attr → a `Reflect`
/// `ColliderScaleSubdivisions(u32)` component → a `Changed<{component,Transform}>`-
/// gated system, ordered `.after` Avian's `update_collider_scale`, that re-applies
/// `set_scale` with the authored count (overriding Avian's `10` only for scaled
/// round shapes). Blocked on Avian: while it hardcodes `10`, any runtime scale
/// edit re-clobbers our value, so a clean realtime story needs Avian's knob first.
fn apply_collider_scale(mut collider: Collider, scale: Vec3) -> Collider {
    collider.set_scale(scale.as_dvec3(), 10);
    collider
}
