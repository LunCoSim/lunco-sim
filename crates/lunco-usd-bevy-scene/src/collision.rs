//! Composed USD collision geometry and placement envelopes.
//!
//! This is a render-free scene contract. It reads the same standard USD
//! collision schemas and primitive dimensions used by the visual and Avian
//! projections, but does not depend on either implementation. Spawn, query,
//! and authoring code can therefore derive one placement envelope without
//! importing the large visual adapter.

use bevy::prelude::{Quat, Transform, Vec3};
use lunco_usd_bevy_core::{
    effective_purpose, local_transform_at, Purpose, StageView, UsdReadObject,
};
use openusd::sdf::Path as SdfPath;

use crate::{
    read_primitive_axis, read_shape_dims, read_usd_mesh_indexed, usd_axis_to_quat, ShapeDims,
};

/// Small gap (metres) left between an asset's lowest collision point and the
/// terrain at spawn. Physics is held until the terrain collider is ready, so
/// the object settles this last gap gently.
pub const SPAWN_GROUND_CLEARANCE: f64 = 0.05;

/// Axis-aligned bounding box of a composed USD asset's collision geometry, in
/// the asset root's reference frame.
///
/// Walks the composed USD stage from `root_prim` and, for every active gprim
/// that applies the standard `UsdPhysicsCollisionAPI` and whose
/// `physics:collisionEnabled` is not `false`, folds that shape's local bounding
/// box into a running min/max. Nested rigid bodies and authored vehicle wheels
/// are included because this is the placement envelope for the complete
/// composed asset. Shape dimensions and native mesh points come from the
/// shared scene readers, so the box cannot drift from the corresponding
/// visual or Avian geometry.
///
/// Returns `Ok(None)` when no collision geometry is found. Malformed authored
/// collision data is an error, not an empty footprint: callers must not replace
/// it with a spawn heuristic.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectAabb {
    pub min: bevy::math::DVec3,
    pub max: bevy::math::DVec3,
}

/// A composed collision tree could not provide a trustworthy placement AABB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollisionAabbError {
    InvalidRootPath(String),
    MalformedTransform { prim: String },
    InvalidCollisionEnabled { prim: String },
    MalformedPrimitive { prim: String, type_name: String },
}

impl std::fmt::Display for CollisionAabbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRootPath(path) => write!(f, "invalid collision root path {path}"),
            Self::MalformedTransform { prim } => {
                write!(f, "malformed authored transform at {prim}")
            }
            Self::InvalidCollisionEnabled { prim } => {
                write!(f, "invalid authored physics:collisionEnabled at {prim}")
            }
            Self::MalformedPrimitive { prim, type_name } => {
                write!(f, "malformed {type_name} collision data at {prim}")
            }
        }
    }
}

impl std::error::Error for CollisionAabbError {}

impl ObjectAabb {
    /// Half-width along X (metres).
    pub fn half_w(&self) -> f64 {
        (self.max.x - self.min.x) * 0.5
    }

    /// Half-length along Z (metres).
    pub fn half_l(&self) -> f64 {
        (self.max.z - self.min.z) * 0.5
    }

    /// Root origin to the lowest collision point.
    pub fn rest_depth(&self) -> f64 {
        -self.min.y
    }
}

/// Derive the [`ObjectAabb`] of an asset by walking the composed USD stage from
/// `root_prim` (for example, `"/DescentLander"`).
pub fn collision_aabb(
    reader: &StageView<'_>,
    root_prim: &str,
) -> Result<Option<ObjectAabb>, CollisionAabbError> {
    let root = SdfPath::new(root_prim)
        .map_err(|_| CollisionAabbError::InvalidRootPath(root_prim.to_owned()))?;
    let root_tf = collision_local_transform(reader, &root)?;
    let mut candidates = vec![(root.clone(), root_tf)];
    gather_collision_aabb_candidates(reader, &root, root_tf, &mut candidates)?;
    let has_proxy = candidates
        .iter()
        .any(|(path, _)| effective_purpose(reader, path) == Purpose::Proxy);
    let mut acc: Option<(bevy::math::DVec3, bevy::math::DVec3)> = None;
    for (path, world_tf) in candidates {
        if UsdReadObject::text(reader, &path, "lunco:triggerZone")
            .is_some_and(|zone| !zone.trim().is_empty())
        {
            continue;
        }
        let purpose = effective_purpose(reader, &path);
        if purpose == Purpose::Guide || (has_proxy && purpose == Purpose::Render) {
            continue;
        }
        if !UsdReadObject::has_api_schema(
            reader,
            &path,
            openusd::schemas::physics::tokens::API_COLLISION,
        ) {
            continue;
        }
        let collides = match UsdReadObject::boolean(reader, &path, "physics:collisionEnabled") {
            Some(value) => value,
            None if UsdReadObject::has_authored_attribute(
                reader,
                &path,
                "physics:collisionEnabled",
            ) =>
            {
                return Err(CollisionAabbError::InvalidCollisionEnabled {
                    prim: path.as_str().to_owned(),
                });
            }
            None => true,
        };
        if !collides {
            continue;
        }
        let ty = UsdReadObject::type_name(reader, &path).unwrap_or_default();
        let corners = local_shape_corners(reader, &path, &ty).ok_or_else(|| {
            CollisionAabbError::MalformedPrimitive {
                prim: path.as_str().to_owned(),
                type_name: ty.clone(),
            }
        })?;
        for corner in corners {
            let world = world_tf.transform_point(corner.as_vec3()).as_dvec3();
            match acc.as_mut() {
                Some((min, max)) => {
                    *min = min.min(world);
                    *max = max.max(world);
                }
                None => acc = Some((world, world)),
            }
        }
    }
    Ok(acc.map(|(min, max)| ObjectAabb { min, max }))
}

/// Derive the composed geometry AABB of one USD shape prim in canonical stage
/// coordinates.
pub fn prim_geometry_aabb(
    reader: &StageView<'_>,
    prim_path: &str,
) -> Result<Option<ObjectAabb>, CollisionAabbError> {
    let path = SdfPath::new(prim_path)
        .map_err(|_| CollisionAabbError::InvalidRootPath(prim_path.to_owned()))?;
    let Some(type_name) = UsdReadObject::type_name(reader, &path) else {
        return Ok(None);
    };
    if !matches!(
        type_name.as_str(),
        "Mesh" | "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane"
    ) {
        return Ok(None);
    }
    let transform = geometry_world_transform(reader, &path)?;
    let corners = local_shape_corners(reader, &path, &type_name).ok_or_else(|| {
        CollisionAabbError::MalformedPrimitive {
            prim: path.as_str().to_owned(),
            type_name: type_name.clone(),
        }
    })?;
    let mut acc: Option<(bevy::math::DVec3, bevy::math::DVec3)> = None;
    for corner in corners {
        let world = transform.transform_point(corner.as_vec3()).as_dvec3();
        match acc.as_mut() {
            Some((min, max)) => {
                *min = min.min(world);
                *max = max.max(world);
            }
            None => acc = Some((world, world)),
        }
    }
    Ok(acc.map(|(min, max)| ObjectAabb { min, max }))
}

/// Read a collision-tree transform while distinguishing USD's identity for an
/// unauthored xform stack from a malformed authored stack.
fn collision_local_transform(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Transform, CollisionAabbError> {
    match local_transform_at(reader, path, 0.0) {
        Ok(Some(transform)) => Ok(transform),
        Ok(None) => Ok(Transform::IDENTITY),
        Err(_) => Err(CollisionAabbError::MalformedTransform {
            prim: path.as_str().to_owned(),
        }),
    }
}

/// Fold local transforms from the stage root to one shape.
fn geometry_world_transform(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<Transform, CollisionAabbError> {
    if !UsdReadObject::has_prim(reader, path) {
        return Err(CollisionAabbError::InvalidRootPath(
            path.as_str().to_owned(),
        ));
    }
    let mut chain = Vec::new();
    let mut current = Some(path.clone());
    while let Some(prim) = current {
        if prim.is_abs_root() {
            break;
        }
        chain.push(prim.clone());
        current = prim.parent();
    }
    let mut transform = Transform::IDENTITY;
    for prim in chain.iter().rev() {
        transform = transform.mul_transform(collision_local_transform(reader, prim)?);
    }
    Ok(transform)
}

/// DFS helper for [`collision_aabb`]. Transforms are composed in the root
/// frame, crossing nested rigid-body and wheel ownership boundaries.
fn gather_collision_aabb_candidates(
    reader: &StageView<'_>,
    path: &SdfPath,
    world_tf: Transform,
    out: &mut Vec<(SdfPath, Transform)>,
) -> Result<(), CollisionAabbError> {
    for child in UsdReadObject::children(reader, path) {
        if !UsdReadObject::is_active(reader, &child) {
            continue;
        }
        let local = collision_local_transform(reader, &child)?;
        let child_world = world_tf * local;
        out.push((child.clone(), child_world));
        gather_collision_aabb_candidates(reader, &child, child_world, out)?;
    }
    Ok(())
}

/// The 8 corners of a primitive shape's local bounding box, centred at its
/// origin. `None` means the authored geometry cannot provide a trustworthy
/// envelope. Round shapes are rotated onto their authored standard `axis`.
fn local_shape_corners(
    reader: &StageView<'_>,
    path: &SdfPath,
    ty: &str,
) -> Option<Vec<bevy::math::DVec3>> {
    if ty == "Mesh" {
        let approximation = UsdReadObject::text(reader, path, "physics:approximation");
        if approximation
            .as_deref()
            .is_some_and(|value| !matches!(value, "none" | "convexHull" | "convexDecomposition"))
            || (approximation.is_none()
                && UsdReadObject::has_authored_attribute(reader, path, "physics:approximation"))
        {
            return None;
        }
        let (vertices, _) = read_usd_mesh_indexed(reader, path)?;
        return Some(
            vertices
                .into_iter()
                .map(|[x, y, z]| bevy::math::DVec3::new(x as f64, y as f64, z as f64))
                .collect(),
        );
    }
    let (half, axial) = match read_shape_dims(reader, path, ty)? {
        ShapeDims::Cube { size } => (bevy::math::DVec3::splat(size * 0.5), false),
        ShapeDims::Sphere { radius } => (bevy::math::DVec3::splat(radius), false),
        ShapeDims::Cylinder { radius, height } | ShapeDims::Cone { radius, height } => {
            (bevy::math::DVec3::new(radius, height * 0.5, radius), true)
        }
        ShapeDims::Capsule { radius, height } => (
            bevy::math::DVec3::new(radius, height * 0.5 + radius, radius),
            true,
        ),
        ShapeDims::Plane { width, length } => (
            bevy::math::DVec3::new(width * 0.5, 0.0005, length * 0.5),
            false,
        ),
    };
    let axis_q = if axial {
        read_primitive_axis(reader, path, ty)
            .and_then(|axis| usd_axis_to_quat(&axis))
            .unwrap_or(Quat::IDENTITY)
    } else {
        Quat::IDENTITY
    };
    let mut corners = Vec::with_capacity(8);
    for sx in [-1.0_f64, 1.0] {
        for sy in [-1.0_f64, 1.0] {
            for sz in [-1.0_f64, 1.0] {
                let local = axis_q
                    * Vec3::new(
                        (half.x * sx) as f32,
                        (half.y * sy) as f32,
                        (half.z * sz) as f32,
                    );
                corners.push(local.as_dvec3());
            }
        }
    }
    Some(corners)
}
