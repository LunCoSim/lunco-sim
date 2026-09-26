//! Composed USD collision geometry and placement envelopes.
//!
//! This is a render-free scene contract. It reads the same standard USD
//! collision schemas and primitive dimensions used by the visual and Avian
//! projections, and uses Avian's shared capability contract when deciding
//! which mesh approximation has a realizable collision envelope. Spawn, query,
//! and authoring code can therefore derive one placement envelope without
//! importing the large visual adapter or duplicating its supported modes.

use bevy::math::{DMat4, DQuat, DVec3};
use lunco_usd_avian_contracts::AvianMeshApproximation;
use lunco_usd_bevy_stage::{
    Purpose, StageView, UsdReadObject, effective_purpose, stage_convention,
};
use openusd::schemas::physics::CollisionApprox;
use openusd::sdf::Path as SdfPath;

use crate::{
    ShapeDims, UsdGeomAxis, read_mesh_collision_approximation, read_shape_dims,
    read_usd_mesh_indexed, usd_plane_surface_vertices,
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
/// shared scene readers. The fidelity field marks bounds that conservatively
/// enclose a convex decomposition's source mesh or a transformed curved
/// primitive's local bounds; the exact collider envelope may be tighter.
///
/// Returns `Ok(None)` when no collision geometry is found. Malformed authored
/// collision data is an error, not an empty footprint: callers must not replace
/// it with a spawn heuristic.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectAabb {
    pub min: bevy::math::DVec3,
    pub max: bevy::math::DVec3,
    /// Whether the bounds exactly match the collider or conservatively
    /// enclose source geometry or transformed local shape bounds.
    pub fidelity: CollisionBoundsFidelity,
}

/// Accuracy contract for a composed collision AABB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollisionBoundsFidelity {
    /// The AABB encloses the extrema of the collider geometry.
    #[default]
    Exact,
    /// The AABB conservatively encloses source geometry or transformed local
    /// bounds; the exact collider envelope can be tighter.
    ConservativeGeometryEnvelope,
}

/// A composed collision tree could not provide a trustworthy placement AABB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollisionAabbError {
    InvalidRootPath(String),
    MalformedTransform {
        prim: String,
    },
    InvalidCollisionEnabled {
        prim: String,
    },
    MalformedPrimitive {
        prim: String,
        type_name: String,
    },
    InvalidApproximation {
        prim: String,
        value: String,
    },
    UnsupportedApproximation {
        prim: String,
        approximation: CollisionApprox,
    },
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
            Self::InvalidApproximation { prim, value } => write!(
                f,
                "{prim} has invalid authored physics:approximation token `{value}`"
            ),
            Self::UnsupportedApproximation {
                prim,
                approximation,
            } => write!(
                f,
                "{prim} uses unsupported physics:approximation `{}` for exact collision geometry",
                approximation.as_token()
            ),
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
    let root_tf = collision_local_matrix(reader, &root)?;
    let mut candidates = vec![(root.clone(), root_tf)];
    gather_collision_aabb_candidates(reader, &root, root_tf, &mut candidates)?;
    let has_proxy = candidates
        .iter()
        .any(|(path, _)| effective_purpose(reader, path) == Purpose::Proxy);
    let mut acc: Option<(bevy::math::DVec3, bevy::math::DVec3)> = None;
    let mut fidelity = CollisionBoundsFidelity::Exact;
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
        let (corners, approximation) = local_shape_corners(reader, &path, &ty, true)?;
        if approximation == Some(AvianMeshApproximation::ConvexDecomposition)
            || matches!(ty.as_str(), "Sphere" | "Cylinder" | "Cone" | "Capsule")
        {
            fidelity = CollisionBoundsFidelity::ConservativeGeometryEnvelope;
        }
        for corner in corners {
            let world = world_tf.transform_point3(corner);
            match acc.as_mut() {
                Some((min, max)) => {
                    *min = min.min(world);
                    *max = max.max(world);
                }
                None => acc = Some((world, world)),
            }
        }
    }
    Ok(acc.map(|(min, max)| ObjectAabb { min, max, fidelity }))
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
    let transform = geometry_world_matrix_d(reader, &path)?;
    let (corners, _) = local_shape_corners(reader, &path, &type_name, false)?;
    let mut acc: Option<(bevy::math::DVec3, bevy::math::DVec3)> = None;
    for corner in corners {
        let world = transform.transform_point3(corner);
        match acc.as_mut() {
            Some((min, max)) => {
                *min = min.min(world);
                *max = max.max(world);
            }
            None => acc = Some((world, world)),
        }
    }
    Ok(acc.map(|(min, max)| ObjectAabb {
        min,
        max,
        fidelity: if matches!(
            type_name.as_str(),
            "Sphere" | "Cylinder" | "Cone" | "Capsule"
        ) {
            CollisionBoundsFidelity::ConservativeGeometryEnvelope
        } else {
            CollisionBoundsFidelity::Exact
        },
    }))
}

/// Read a collision-tree transform while distinguishing USD's identity for an
/// unauthored xform stack from a malformed authored stack.
fn collision_local_matrix(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<DMat4, CollisionAabbError> {
    match lunco_usd_bevy_stage::local_transform_matrix_d_at(reader, path, 0.0) {
        Ok(Some(transform)) => Ok(transform),
        Ok(None) => Ok(DMat4::IDENTITY),
        Err(_) => Err(CollisionAabbError::MalformedTransform {
            prim: path.as_str().to_owned(),
        }),
    }
}

/// Fold local transforms from the stage root to one shape.
/// Compose the USD prim's local transforms into canonical stage coordinates
/// without narrowing authored double-precision matrices.
pub fn geometry_world_matrix_d(
    reader: &StageView<'_>,
    path: &SdfPath,
) -> Result<DMat4, CollisionAabbError> {
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
    let mut transform = DMat4::IDENTITY;
    for prim in chain.iter().rev() {
        let local = lunco_usd_bevy_stage::local_transform_matrix_d_at(reader, prim, 0.0)
            .map_err(|_| CollisionAabbError::MalformedTransform {
                prim: prim.as_str().to_owned(),
            })?
            .unwrap_or(DMat4::IDENTITY);
        transform *= local;
    }
    Ok(transform)
}

/// DFS helper for [`collision_aabb`]. Transforms are composed in the root
/// frame, crossing nested rigid-body and wheel ownership boundaries.
fn gather_collision_aabb_candidates(
    reader: &StageView<'_>,
    path: &SdfPath,
    world_tf: DMat4,
    out: &mut Vec<(SdfPath, DMat4)>,
) -> Result<(), CollisionAabbError> {
    for child in UsdReadObject::children(reader, path) {
        if !UsdReadObject::is_active(reader, &child) {
            continue;
        }
        let local = collision_local_matrix(reader, &child)?;
        let child_world = world_tf * local;
        out.push((child.clone(), child_world));
        gather_collision_aabb_candidates(reader, &child, child_world, out)?;
    }
    Ok(())
}

/// Corners of a primitive's local geometry or exact supported collider bounds,
/// centred at its origin. Round shapes and finite planes are rotated onto
/// their authored standard `axis`.
fn local_shape_corners(
    reader: &StageView<'_>,
    path: &SdfPath,
    ty: &str,
    collision_geometry: bool,
) -> Result<(Vec<bevy::math::DVec3>, Option<AvianMeshApproximation>), CollisionAabbError> {
    let mut approximation = None;
    if ty == "Mesh" {
        if collision_geometry {
            let selected = read_mesh_collision_approximation(reader, path).map_err(|error| {
                CollisionAabbError::InvalidApproximation {
                    prim: error.prim,
                    value: error.value,
                }
            })?;
            approximation = Some(AvianMeshApproximation::try_from(selected).map_err(
                |unsupported| CollisionAabbError::UnsupportedApproximation {
                    prim: path.as_str().to_owned(),
                    approximation: unsupported,
                },
            )?);
        }
        let (vertices, _) = read_usd_mesh_indexed(reader, path).ok_or_else(|| {
            CollisionAabbError::MalformedPrimitive {
                prim: path.as_str().to_owned(),
                type_name: ty.to_owned(),
            }
        })?;
        let vertices = vertices
            .into_iter()
            .map(|[x, y, z]| DVec3::new(x as f64, y as f64, z as f64))
            .collect::<Vec<_>>();
        let vertices = if approximation == Some(AvianMeshApproximation::BoundingCube) {
            let mut min = DVec3::splat(f64::INFINITY);
            let mut max = DVec3::splat(f64::NEG_INFINITY);
            for vertex in vertices {
                min = min.min(vertex);
                max = max.max(vertex);
            }
            (0..8)
                .map(|bits| {
                    DVec3::new(
                        if bits & 1 == 0 { min.x } else { max.x },
                        if bits & 2 == 0 { min.y } else { max.y },
                        if bits & 4 == 0 { min.z } else { max.z },
                    )
                })
                .collect()
        } else {
            vertices
        };
        return Ok((vertices, approximation));
    }
    let dimensions = read_shape_dims(reader, path, ty).ok_or_else(|| {
        CollisionAabbError::MalformedPrimitive {
            prim: path.as_str().to_owned(),
            type_name: ty.to_owned(),
        }
    })?;
    let (half, axis) = match dimensions {
        ShapeDims::Cube { size } => (DVec3::splat(size * 0.5), None),
        ShapeDims::Sphere { radius } => (DVec3::splat(radius), None),
        ShapeDims::Cylinder {
            radius,
            height,
            axis,
        }
        | ShapeDims::Cone {
            radius,
            height,
            axis,
        } => (DVec3::new(radius, height * 0.5, radius), Some(axis)),
        ShapeDims::Capsule {
            radius,
            height,
            axis,
        } => (
            DVec3::new(radius, height * 0.5 + radius, radius),
            Some(axis),
        ),
        ShapeDims::Plane {
            width,
            length,
            axis,
        } => {
            let axis_q = stage_convention(reader)
                .map_err(|_| CollisionAabbError::MalformedPrimitive {
                    prim: path.as_str().to_owned(),
                    type_name: ty.to_owned(),
                })?
                .orient_d(usd_axis_to_dquat(axis));
            let corners = usd_plane_surface_vertices(width, length, axis)
                .into_iter()
                .map(|[x, y, z]| axis_q * DVec3::new(x, y, z))
                .collect();
            return Ok((corners, approximation));
        }
    };
    let axis_q = match axis {
        Some(axis) => stage_convention(reader)
            .map_err(|_| CollisionAabbError::MalformedPrimitive {
                prim: path.as_str().to_owned(),
                type_name: ty.to_owned(),
            })?
            .orient_d(usd_axis_to_dquat(axis)),
        None => DQuat::IDENTITY,
    };
    let mut corners = Vec::with_capacity(8);
    for sx in [-1.0_f64, 1.0] {
        for sy in [-1.0_f64, 1.0] {
            for sz in [-1.0_f64, 1.0] {
                let local = axis_q * DVec3::new(half.x * sx, half.y * sy, half.z * sz);
                corners.push(local);
            }
        }
    }
    Ok((corners, approximation))
}

fn usd_axis_to_dquat(axis: UsdGeomAxis) -> DQuat {
    match axis {
        UsdGeomAxis::X => DQuat::from_rotation_arc(DVec3::Y, DVec3::X),
        UsdGeomAxis::Y => DQuat::IDENTITY,
        UsdGeomAxis::Z => DQuat::from_rotation_arc(DVec3::Y, DVec3::Z),
    }
}
