use avian3d::physics_transform::{Position, Rotation};
use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_usd_avian_contracts::AvianMeshApproximation;
use lunco_usd_bevy_mesh::{NurbsCollisionTessellation, build_nurbs_collision_mesh_from_usd};
use lunco_usd_bevy_scene::{
    ShapeDims, read_mesh_collision_approximation, read_primitive_axis, read_shape_dims,
    read_usd_mesh_indexed, read_usd_mesh_topology, usd_axis_to_quat, usd_plane_surface_vertices,
};
use lunco_usd_bevy_stage::{Purpose, TransformReadError, effective_purpose, local_transform_at};
use openusd::schemas::physics::CollisionApprox;
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::{Path as SdfPath, Value as SdfValue};

use crate::read_authored_bool_or_default;

/// Checks if a USD prim has a specific API schema applied.
/// Collects collider shapes from all descendant prims of a compound body root,
/// reading directly from the USD stage.
///
/// Returns a list of `(Position, Rotation, Collider)` tuples for `Collider::compound()`.
#[derive(Debug)]
pub enum ColliderProjectionError {
    Transform(TransformReadError),
    Backend {
        prim: String,
        detail: String,
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

impl std::fmt::Display for ColliderProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transform(error) => error.fmt(f),
            Self::Backend { prim, detail } => write!(f, "{prim}: {detail}"),
            Self::InvalidApproximation { prim, value } => write!(
                f,
                "{prim}: invalid authored {} token `{value}`",
                ptok::A_APPROXIMATION
            ),
            Self::UnsupportedApproximation {
                prim,
                approximation,
            } => write!(
                f,
                "{prim}: Avian cannot realize the authored {} mode `{}`",
                ptok::A_APPROXIMATION,
                approximation.as_token()
            ),
        }
    }
}

impl std::error::Error for ColliderProjectionError {}

/// The collider builder's complete non-error result. The caller decides
/// whether an unsupported prim is irrelevant to physics or violates an
/// authored collision contract, and whether an external mesh is still loading.
#[derive(Debug)]
pub enum ColliderBuildOutcome {
    Built(Collider),
    UnsupportedGeometry { type_name: Option<String> },
    DeferredMeshAsset,
}

/// One geometric part of an Avian collider after USD approximation cooking.
#[derive(Clone, Debug)]
pub enum ColliderGeometryPart {
    /// Triangle topology used by a static triangle-mesh collider.
    TriangleMesh {
        /// Vertices in the collider prim's local frame.
        vertices: Vec<[f64; 3]>,
        /// Triangle vertex indices.
        triangles: Vec<[u32; 3]>,
    },
    /// Vertices of one convex collider part after the backend cook.
    ConvexHull {
        /// Hull vertices in the collider prim's local frame.
        vertices: Vec<[f64; 3]>,
    },
}

/// Collision geometry produced by the same USD-to-Avian reader used at runtime.
#[derive(Clone, Debug)]
pub struct AuthoredColliderGeometry {
    /// Standard USD mesh approximation; primitive colliders have no mesh mode.
    pub approximation: Option<CollisionApprox>,
    /// Cooked shape parts. Convex decomposition produces one entry per hull.
    pub parts: Vec<ColliderGeometryPart>,
}

/// Project one explicitly authored USD collision prim into an Avian collider.
///
/// This is the shared projection boundary for specialized realizations such as
/// physical vehicle wheels. It accepts only a prim carrying
/// `PhysicsCollisionAPI` and a supported USD geometry type. In particular, it
/// never derives a collider from a domain parameter such as wheel radius or
/// width. Missing or unsupported authored geometry is an error at the owner.
pub fn authored_collider_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<Collider, ColliderProjectionError> {
    let scale = local_transform_at(reader, sdf_path, 0.0)
        .map_err(ColliderProjectionError::Transform)?
        .map_or(Vec3::ONE, |transform| transform.scale);
    authored_collider_from_usd_at_scale(reader, sdf_path, scale)
}

/// Project one explicitly authored collision prim using its caller-supplied
/// composed scale. Compound-body readers use this when intermediate Xforms
/// contribute scale without producing a collider entity of their own.
pub fn authored_collider_from_usd_at_scale(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
    scale: Vec3,
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

    let collider = match build_collider_from_usd_at_scale(reader, sdf_path, scale)? {
        ColliderBuildOutcome::Built(collider) => collider,
        ColliderBuildOutcome::UnsupportedGeometry { type_name } => {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: format!(
                    "{} has PhysicsCollisionAPI but `{}` is not a supported geometric collider",
                    sdf_path,
                    type_name.as_deref().unwrap_or("unknown prim")
                ),
            });
        }
        ColliderBuildOutcome::DeferredMeshAsset => {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: "authored collider geometry is an external mesh that has not loaded"
                    .to_owned(),
            });
        }
    };
    if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "collider local bounds are not finite, ordered, or f32-representable"
                .to_owned(),
        });
    }
    Ok(collider)
}

/// Cook a mesh or cube collision prim and return the geometry Avian will use.
/// Each convex-decomposition member remains a separate part so callers cannot
/// accidentally fill the gaps between disconnected hulls.
pub fn authored_collider_geometry_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<Option<AuthoredColliderGeometry>, ColliderProjectionError> {
    if !reader.has_api_schema(sdf_path, ptok::API_COLLISION) {
        return Ok(None);
    }
    match read_authored_bool_or_default(reader, sdf_path, ptok::A_COLLISION_ENABLED, true) {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(()) => {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: format!("malformed {}", ptok::A_COLLISION_ENABLED),
            });
        }
    }
    let Some(type_name) = reader.type_name(sdf_path) else {
        return Ok(None);
    };
    if !matches!(type_name.as_str(), "Mesh" | "Cube") {
        return Ok(None);
    }

    // The stage transform is applied by the query after this local cook. This
    // avoids applying the prim scale once in Avian and again in the composed
    // transform used to return canonical-stage coordinates.
    let collider = authored_collider_from_usd_at_scale(reader, sdf_path, Vec3::ONE)?;
    let approximation = if type_name == "Mesh" {
        Some(
            read_mesh_collision_approximation(reader, sdf_path).map_err(|error| {
                ColliderProjectionError::InvalidApproximation {
                    prim: error.prim,
                    value: error.value,
                }
            })?,
        )
    } else {
        None
    };
    let parts = collider_geometry_parts(&collider, sdf_path)?;
    Ok(Some(AuthoredColliderGeometry {
        approximation,
        parts,
    }))
}

fn collider_geometry_parts(
    collider: &Collider,
    prim: &SdfPath,
) -> Result<Vec<ColliderGeometryPart>, ColliderProjectionError> {
    let shape = collider.shape();
    if let Some(mesh) = shape.as_trimesh() {
        return Ok(vec![ColliderGeometryPart::TriangleMesh {
            vertices: mesh
                .vertices()
                .iter()
                .map(|point| [point.x as f64, point.y as f64, point.z as f64])
                .collect(),
            triangles: mesh.indices().to_vec(),
        }]);
    }
    if let Some(hull) = shape.as_convex_polyhedron() {
        return Ok(vec![ColliderGeometryPart::ConvexHull {
            vertices: hull
                .points()
                .iter()
                .map(|point| [point.x as f64, point.y as f64, point.z as f64])
                .collect(),
        }]);
    }
    if let Some(cuboid) = shape.as_cuboid() {
        let half = cuboid.half_extents;
        let vertices = (0..8)
            .map(|bits| {
                [
                    (if bits & 1 == 0 { -half.x } else { half.x }) as f64,
                    (if bits & 2 == 0 { -half.y } else { half.y }) as f64,
                    (if bits & 4 == 0 { -half.z } else { half.z }) as f64,
                ]
            })
            .collect();
        return Ok(vec![ColliderGeometryPart::ConvexHull { vertices }]);
    }
    if let Some(compound) = shape.as_compound() {
        let mut parts = Vec::with_capacity(compound.shapes().len());
        for (pose, child) in compound.shapes() {
            let Some(hull) = child.as_convex_polyhedron() else {
                return Err(ColliderProjectionError::Backend {
                    prim: prim.to_string(),
                    detail: "Avian convex decomposition contains a non-convex part".to_owned(),
                });
            };
            let vertices = hull
                .points()
                .iter()
                .map(|point| {
                    let point = pose.transform_point(*point);
                    [point.x as f64, point.y as f64, point.z as f64]
                })
                .collect();
            parts.push(ColliderGeometryPart::ConvexHull { vertices });
        }
        if !parts.is_empty() {
            return Ok(parts);
        }
    }
    Err(ColliderProjectionError::Backend {
        prim: prim.to_string(),
        detail: "Avian produced a collider shape that has no mesh or convex-vertex query"
            .to_owned(),
    })
}

pub fn collect_child_colliders_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    parent_path: &SdfPath,
) -> Result<Vec<(Position, Rotation, Collider)>, ColliderProjectionError> {
    let mut shapes = Vec::new();
    let convention = lunco_usd_bevy_stage::stage_convention(reader).map_err(|_| {
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
                return Err(ColliderProjectionError::Backend {
                    prim: child_path.to_string(),
                    detail: format!(
                        "malformed {}; compound collider would otherwise be incomplete",
                        ptok::A_COLLISION_ENABLED
                    ),
                });
            }
        };
        if !child_collision {
            continue;
        }

        // For axis-aligned primitive children, fold the authored axis into the
        // child's compound-local rotation so the canonical collider lines
        // up with the authored axis (mirrors what lunco-usd-bevy does
        // for the entity Transform — same canonical `usd_axis_to_quat`).
        // The body root is different: its axis is already on the ECS body
        // transform, so applying it again inside the compound would rotate the
        // root shape twice.
        let is_body_shape = child_path.as_str() == parent_path.as_str();
        if !is_body_shape {
            if let Some(ty) = reader.type_name(&child_path) {
                if matches!(ty.as_str(), "Cylinder" | "Cone" | "Capsule" | "Plane") {
                    let axis_tok =
                        read_primitive_axis(reader, &child_path, &ty).ok_or_else(|| {
                            ColliderProjectionError::Backend {
                                prim: child_path.to_string(),
                                detail: format!("invalid authored {ty} axis"),
                            }
                        })?;
                    // Pre-rotate by the stage convention: the `axis` token names an
                    // axis of the STAGE's frame while the collider is built in the
                    // canonical one (identical to what usd-bevy does for the visual
                    // Transform, so mesh and collider can't disagree on a Z-up stage).
                    let q_axis = convention.orient(usd_axis_to_quat(axis_tok));
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
        let collider = match build_collider_from_usd_at_scale(reader, &child_path, scale)? {
            ColliderBuildOutcome::Built(collider) => collider,
            ColliderBuildOutcome::UnsupportedGeometry { type_name } => {
                return Err(ColliderProjectionError::Backend {
                    prim: child_path.to_string(),
                    detail: format!(
                        "{} has PhysicsCollisionAPI but `{}` has no supported collider projection",
                        child_path,
                        type_name.as_deref().unwrap_or("unknown prim")
                    ),
                });
            }
            ColliderBuildOutcome::DeferredMeshAsset => {
                return Err(ColliderProjectionError::Backend {
                    prim: child_path.to_string(),
                    detail: "compound body collider geometry cannot be deferred while an external mesh loads"
                        .to_owned(),
                });
            }
        };
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
                detail:
                    "collider pose or local bounds are not finite, ordered, or f32-representable"
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
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
///   cylinder is Y-axial and its plane surface is +Y-normal; authored axes are
///   honored by the entity transform or, for compound children, by
///   `collect_child_colliders_from_usd`.
///
/// `UsdGeomCube` is cubic: `size` is its only dimension. A non-uniform box is
/// `size` plus a non-uniform `xformOp:scale`, which the scale tail applies.
pub fn build_collider_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<ColliderBuildOutcome, ColliderProjectionError> {
    let scale = local_transform_at(reader, sdf_path, 0.0)
        .map_err(ColliderProjectionError::Transform)?
        .map_or(Vec3::ONE, |transform| transform.scale);
    if !lunco_physics::avian_backend_vector_is_valid(scale.as_dvec3()) {
        return Err(ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: "collider scale is not finite or f32-representable".to_owned(),
        });
    }
    build_collider_from_usd_at_scale(reader, sdf_path, scale)
}

/// Build a collider with a scale already composed from the owning body frame to
/// the geometry prim. Standalone colliders obtain this from their own local
/// transform; compound children obtain it from [`gather_compound_candidates`],
/// because intermediate USD Xforms have no corresponding Avian collider entity.
pub fn build_collider_from_usd_at_scale(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
    scale: Vec3,
) -> Result<ColliderBuildOutcome, ColliderProjectionError> {
    let Some(ty) = reader.type_name(sdf_path) else {
        return Ok(ColliderBuildOutcome::UnsupportedGeometry { type_name: None });
    };

    // Native UsdGeomMesh → static triangle-mesh collider, decoded from the
    // SAME `points`/`faceVertexIndices` `lunco-usd-bevy` renders (one geometry
    // source, so collider and visual can't drift). `set_scale` on a trimesh
    // scales its vertices exactly (no convex-hull tessellation), so the shared
    // scale tail applies unchanged.
    if ty == "Mesh" {
        let Some((verts, tris)) = read_usd_mesh_indexed(reader, sdf_path) else {
            // Only an explicitly mesh-loaded asset may defer collider creation.
            // Missing native USD points/topology is malformed geometry and
            // must not be confused with an asynchronous asset load.
            if reader.text(sdf_path, "lunco:assetMode").as_deref() == Some("mesh") {
                return Ok(ColliderBuildOutcome::DeferredMeshAsset);
            }
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: "native UsdGeomMesh has no valid points and indexed face topology"
                    .to_owned(),
            });
        };
        if reader.has_api_schema(sdf_path, "LunCoDerivedGeometryAPI") {
            validate_derived_nurbs_proxy(reader, sdf_path, &verts, &tris)?;
        }
        let verts: Vec<DVec3> = verts
            .into_iter()
            .map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64))
            .collect();
        // Parse the full standard USD enum, then narrow it through the shared
        // Avian capability contract. Runtime projection and authoring queries
        // use the same mapping, so unsupported standard tokens cannot be
        // advertised by the proxy planner.
        // `physics:approximation` is a property OF `PhysicsMeshCollisionAPI`, so
        // it only means anything when that schema is applied.
        let usd_approximation =
            read_mesh_collision_approximation(reader, sdf_path).map_err(|error| {
                ColliderProjectionError::InvalidApproximation {
                    prim: error.prim,
                    value: error.value,
                }
            })?;
        let approximation =
            AvianMeshApproximation::try_from(usd_approximation).map_err(|approximation| {
                ColliderProjectionError::UnsupportedApproximation {
                    prim: sdf_path.to_string(),
                    approximation,
                }
            })?;
        if approximation.requires_static_or_kinematic_body() {
            if let Some(body) = dynamic_rigid_body_ancestor(reader, sdf_path)? {
                return Err(ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: format!(
                        "triangle-mesh approximation `none` is invalid on dynamic body `{body}`; author a convex mesh approximation"
                    ),
                });
            }
        }
        let collider = match approximation {
            AvianMeshApproximation::ConvexHull => {
                Collider::convex_hull(verts).ok_or_else(|| ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: "authored convexHull approximation could not be built".to_owned(),
                })?
            }
            AvianMeshApproximation::ConvexDecomposition => {
                Collider::convex_decomposition(verts, tris)
            }
            AvianMeshApproximation::TriangleMesh => {
                Collider::try_trimesh(verts, tris).map_err(|error| {
                    ColliderProjectionError::Backend {
                        prim: sdf_path.to_string(),
                        detail: format!("authored triangle mesh could not be built: {error}"),
                    }
                })?
            }
            AvianMeshApproximation::BoundingCube => {
                let mut min = DVec3::splat(f64::INFINITY);
                let mut max = DVec3::splat(f64::NEG_INFINITY);
                for vertex in &verts {
                    min = min.min(*vertex);
                    max = max.max(*vertex);
                }
                if !min.is_finite()
                    || !max.is_finite()
                    || (max.x - min.x) <= f64::EPSILON
                    || (max.y - min.y) <= f64::EPSILON
                    || (max.z - min.z) <= f64::EPSILON
                {
                    return Err(ColliderProjectionError::Backend {
                        prim: sdf_path.to_string(),
                        detail: "authored boundingCube approximation needs nonzero extent on all three local axes".to_owned(),
                    });
                }
                let corners = (0..8)
                    .map(|bits| {
                        DVec3::new(
                            if bits & 1 == 0 { min.x } else { max.x },
                            if bits & 2 == 0 { min.y } else { max.y },
                            if bits & 4 == 0 { min.z } else { max.z },
                        )
                    })
                    .collect();
                Collider::convex_hull(corners).ok_or_else(|| ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: "authored boundingCube approximation could not be built".to_owned(),
                })?
            }
        };
        return Ok(ColliderBuildOutcome::Built(apply_collider_scale(
            collider, scale,
        )));
    }

    // Dimensions (+ their magic defaults) come from the canonical
    // `read_shape_dims` shared with usd-bevy's mesh builder, so the
    // collider can't desync from the visual mesh. Build the INTRINSIC
    // (unscaled) shape; the scale tail below owns scaling.
    let recognized_geometry = matches!(
        ty.as_str(),
        "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane"
    );
    if !recognized_geometry {
        return Ok(ColliderBuildOutcome::UnsupportedGeometry {
            type_name: Some(ty),
        });
    }
    let shape_dims = read_shape_dims(reader, sdf_path, ty.as_str()).ok_or_else(|| {
        ColliderProjectionError::Backend {
            prim: sdf_path.to_string(),
            detail: format!("malformed authored UsdGeom{ty} dimensions or axis"),
        }
    })?;
    let collider = match shape_dims {
        ShapeDims::Cube { size } => Collider::cuboid(size, size, size),
        ShapeDims::Sphere { radius } => Collider::sphere(radius),
        ShapeDims::Cylinder { radius, height, .. } => Collider::cylinder(radius, height),
        ShapeDims::Cone { radius, height, .. } => Collider::cone(radius, height),
        ShapeDims::Capsule { radius, height, .. } => Collider::capsule(radius, height),
        // Preserve the finite authored surface as two coplanar triangles. A
        // synthetic box thickness would create collision volume absent from
        // the USD geometry.
        ShapeDims::Plane {
            width,
            length,
            axis,
        } => {
            let vertices = usd_plane_surface_vertices(width, length, axis)
                .into_iter()
                .map(|[x, y, z]| DVec3::new(x, y, z))
                .collect();
            Collider::try_trimesh(vertices, vec![[0, 1, 2], [0, 2, 3]]).map_err(|error| {
                ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: format!("authored finite UsdGeomPlane could not be built: {error}"),
                }
            })?
        }
    };

    Ok(ColliderBuildOutcome::Built(apply_collider_scale(
        collider, scale,
    )))
}

fn dynamic_rigid_body_ancestor(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &SdfPath,
) -> Result<Option<SdfPath>, ColliderProjectionError> {
    let mut ancestor = Some(prim.clone());
    while let Some(path) = ancestor {
        if reader.has_api_schema(&path, ptok::API_RIGID_BODY) {
            let enabled = match reader.boolean(&path, ptok::A_RIGID_BODY_ENABLED) {
                Some(value) => value,
                None if reader.has_authored_attribute(&path, ptok::A_RIGID_BODY_ENABLED) => {
                    return Err(ColliderProjectionError::Backend {
                        prim: path.to_string(),
                        detail: format!("malformed {}", ptok::A_RIGID_BODY_ENABLED),
                    });
                }
                None => true,
            };
            let kinematic = match reader.boolean(&path, ptok::A_KINEMATIC_ENABLED) {
                Some(value) => value,
                None if reader.has_authored_attribute(&path, ptok::A_KINEMATIC_ENABLED) => {
                    return Err(ColliderProjectionError::Backend {
                        prim: path.to_string(),
                        detail: format!("malformed {}", ptok::A_KINEMATIC_ENABLED),
                    });
                }
                None => false,
            };
            if enabled && !kinematic {
                return Ok(Some(path));
            }
        }
        ancestor = path.parent();
    }
    Ok(None)
}

/// Re-cook a derived NURBS mesh from its composed source and compare both the
/// stored fingerprint and authored proxy geometry. A proxy that was edited or
/// whose source changed is rejected before physics can consume stale data.
fn validate_derived_nurbs_proxy(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    proxy: &SdfPath,
    actual_points: &[[f32; 3]],
    actual_triangles: &[[u32; 3]],
) -> Result<(), ColliderProjectionError> {
    let fail = |detail: String| ColliderProjectionError::Backend {
        prim: proxy.to_string(),
        detail,
    };
    let sources = reader.rel_targets(proxy, "lunco:derived:source");
    let [source] = sources.as_slice() else {
        return Err(fail(
            "LunCoDerivedGeometryAPI needs exactly one lunco:derived:source relationship"
                .to_owned(),
        ));
    };
    if reader.type_name(source).as_deref() != Some("NurbsPatch") {
        return Err(fail(format!(
            "derived collision source `{source}` is missing or is not a UsdGeomNurbsPatch"
        )));
    }
    let read_setting = |name: &str| {
        reader
            .integer(proxy, name)
            .and_then(|value| usize::try_from(value).ok())
    };
    let tessellation = NurbsCollisionTessellation {
        u_subdivisions: read_setting("lunco:derived:nurbsUSubdivisions").ok_or_else(|| {
            fail("derived NURBS U subdivisions are missing or invalid".to_owned())
        })?,
        v_subdivisions: read_setting("lunco:derived:nurbsVSubdivisions").ok_or_else(|| {
            fail("derived NURBS V subdivisions are missing or invalid".to_owned())
        })?,
        trim_curve_samples: read_setting("lunco:derived:trimCurveSamples")
            .ok_or_else(|| fail("derived trim-curve samples are missing or invalid".to_owned()))?,
        trim_grid_subdivisions: read_setting("lunco:derived:trimGridSubdivisions").ok_or_else(
            || fail("derived trim-grid subdivisions are missing or invalid".to_owned()),
        )?,
    };
    if !tessellation.is_valid() {
        return Err(fail(
            "derived NURBS tessellation settings are outside supported ranges".to_owned(),
        ));
    }
    let expected_fingerprint = match reader.attr_value(proxy, "lunco:derived:geometryFingerprint") {
        Some(SdfValue::Uint64(value)) => value,
        _ => {
            return Err(fail(
                "derived geometry fingerprint is missing or is not authored as uint64".to_owned(),
            ));
        }
    };
    let expected =
        build_nurbs_collision_mesh_from_usd(reader, source, tessellation).ok_or_else(|| {
            fail(format!(
                "derived NURBS source `{source}` can no longer be cooked"
            ))
        })?;
    if expected.geometry_fingerprint != expected_fingerprint {
        return Err(fail(format!(
            "derived collision proxy is stale: source `{source}` no longer produces fingerprint {expected_fingerprint}; regenerate it"
        )));
    }
    let expected_triangles: Vec<[u32; 3]> = expected
        .face_vertex_indices
        .chunks_exact(3)
        .map(|indices| [indices[0] as u32, indices[1] as u32, indices[2] as u32])
        .collect();
    let topology = read_usd_mesh_topology(reader, proxy).ok_or_else(|| {
        fail("derived collision proxy has malformed USD mesh topology".to_owned())
    })?;
    let indices_match = topology.face_vertex_counts.len() == expected.face_vertex_counts.len()
        && topology.face_vertex_counts.iter().all(|&count| count == 3)
        && topology.face_vertex_indices == expected.face_vertex_indices;
    let scale = expected
        .points
        .iter()
        .flatten()
        .fold(1.0_f32, |scale, value| scale.max(value.abs()));
    let tolerance = 2.0e-6 * scale;
    let points_match = topology.points.len() == expected.points.len()
        && topology
            .points
            .iter()
            .zip(&expected.points)
            .all(|(actual, expected)| {
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() <= tolerance)
            });
    if !indices_match
        || !points_match
        || actual_points.len() != expected.points.len()
        || actual_triangles != expected_triangles
    {
        return Err(fail(format!(
            "derived collision mesh geometry does not match its NURBS source cook `{source}`; regenerate it"
        )));
    }
    Ok(())
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
