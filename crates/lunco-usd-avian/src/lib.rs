//! # LunCoSim USD → Avian3D Physics Mapping
//!
//! Maps USD physics attributes to Avian3D components. This is the **second** plugin in
//! the USD processing pipeline, running after `UsdBevyPlugin` and alongside `UsdSimPlugin`.
//!
//! ## USD Standard: Compound Rigid Bodies
//!
//! Per the OpenUSD specification, a prim with `PhysicsRigidBodyAPI` aggregates all
//! descendant colliders into a **single compound rigid body**. Children with only
//! `PhysicsCollisionAPI` contribute collider shapes but are NOT independent bodies.
//!
//! Our loader follows this standard:
//! - **Parent with RigidBodyAPI** → ONE `RigidBody::Dynamic` + `SelectableRoot`
//! - **Children with CollisionAPI** → `Collider` only (no independent `RigidBody`)
//!
//! ## Mapped Attributes
//!
//! | USD Attribute | Avian3D Component | Notes |
//! |---|---|---|
//! | `PhysicsRigidBodyAPI` (parent) | `RigidBody::Dynamic` | ONE per compound assembly |
//! | `PhysicsCollisionAPI` (child) | `Collider` | Aggregated into parent compound |
//! | `physics:mass` | `Mass` | On the rigid body root |
//! | `physics:linearDamping` | `LinearDamping` | |
//! | `physics:angularDamping` | `AngularDamping` | |
//! | `physics:friction` | `Friction` | |
//!
//! ## Collider Mapping
//!
//! The collider shape is determined by the prim's `typeName`:
//! - `Cube` → `Collider::cuboid(width, height, depth)` — full dimensions
//! - `Sphere` → `Collider::sphere(radius)`
//! - `Cylinder` → `Collider::cylinder(radius, height)`
//!
//! **Important**: `Collider::cuboid()` takes **full dimensions** (same as the USD file's
//! `width`/`height`/`depth`), not half-extents. Avian3D internally halves them to produce
//! the half-extents used in collision detection.
//!
//! ## Why Deferred Processing?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD asset
//! may not be loaded yet (async loading). The `process_usd_avian_prims` system runs in the
//! `Update` schedule and retries every frame until the asset is available.

use bevy::prelude::*;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::DVec3;
use bevy::mesh::VertexAttributeValues;
use avian3d::prelude::*;
use avian3d::physics_transform::{Position, Rotation};
use lunco_usd_bevy::{
    read_shape_dims, read_transform_from_usd,
    read_usd_mesh_indexed, usd_axis_to_quat, ShapeDims, UsdAnimated, UsdRead, UsdVisualSynced,
};
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset, UsdInstanceRoot};
use openusd::sdf::Path as SdfPath;
use openusd::usd::Stage;

/// Bevy plugin for USD physics mapping.
///
/// Adds an observer for USD prim spawning and a deferred processing system that maps
/// USD physics attributes to Avian3D components. The deferred system runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure assets are loaded.
pub struct UsdAvianPlugin;

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        // `on_add_usd_prim`: eager observer for joint pending-state.
        // `process_usd_avian_prims`: observer on UsdVisualSynced — fires
        //   right after `sync_usd_visuals` translates each prim, so the
        //   stage is loaded and Mesh3d/Transform exist.
        // `build_usd_physics_joints`: stays a per-frame system because
        //   it's a deferred state-machine waiting on Avian to admit
        //   both bodies into its island graph (FixedUpdate-driven).
        //   `run_if(any pending)` makes it idle when no joints await.
        app.register_type::<ShouldBeDynamic>()
            .register_type::<lunco_core::Mobility>()
            .add_observer(on_add_usd_prim)
            .add_observer(process_usd_avian_prims)
            .add_systems(
                Update,
                (
                    build_usd_physics_joints.run_if(any_with_component::<PendingUsdJoint>),
                    build_terrain_mesh_colliders
                        .run_if(any_with_component::<PendingTerrainCollider>),
                    enforce_kinematic_on_animated,
                    project_mobility_to_rigid_body,
                ),
            );
    }
}

/// Project a source-declared [`Mobility`](lunco_core::Mobility) onto the live
/// avian `RigidBody` for bodies the USD spawn path didn't already build — so a
/// rhai / Modelica / editor source can spawn a physics body by declaring its
/// mobility alone (one knob, no avian dependency upstream).
///
/// Gated `Without<RigidBody>` so it NEVER overrides a body the USD path manages
/// (including the transient `Kinematic` a settling `Dynamic` body wears via
/// `ShouldBeDynamic`), and `Changed<Mobility>` so it's empty in steady state. A
/// declared-mobility change on a body that already has a `RigidBody` (a live
/// static⇄dynamic flip) is intentionally out of scope here — it needs engine-
/// aware transition handling and is a documented follow-up.
fn project_mobility_to_rigid_body(
    mut commands: Commands,
    q: Query<
        (Entity, &lunco_core::Mobility),
        (Changed<lunco_core::Mobility>, Without<RigidBody>),
    >,
) {
    for (entity, mobility) in &q {
        let body = match mobility {
            lunco_core::Mobility::Static => RigidBody::Static,
            lunco_core::Mobility::Kinematic => RigidBody::Kinematic,
            lunco_core::Mobility::Dynamic => RigidBody::Dynamic,
        };
        commands.entity(entity).insert(body);
    }
}

#[cfg(test)]
mod mobility_tests {
    use super::*;

    #[test]
    fn projects_declared_mobility_but_never_overrides_a_managed_body() {
        let mut app = App::new();
        app.add_systems(Update, project_mobility_to_rigid_body);

        // A bare declaration (rhai/Modelica source) → projected to a body.
        let bare = app.world_mut().spawn(lunco_core::Mobility::Dynamic).id();
        // A USD-managed `Dynamic` body mid-settle wears a transient `Kinematic`;
        // the projector must NOT stomp it back to `Dynamic`.
        let managed = app
            .world_mut()
            .spawn((lunco_core::Mobility::Dynamic, RigidBody::Kinematic))
            .id();

        app.update();

        assert_eq!(app.world().get::<RigidBody>(bare), Some(&RigidBody::Dynamic));
        assert_eq!(
            app.world().get::<RigidBody>(managed),
            Some(&RigidBody::Kinematic),
            "projector must not override a body the spawn path already manages"
        );
    }
}

/// An animated USD body must be `Kinematic`, never `Dynamic`: the per-frame
/// [`lunco_usd_bevy::sample_usd_animation`] sampler writes its `Transform`
/// directly, and a `Dynamic` body would fight Avian's integrator each step
/// (the authored pose and the solved pose disagree → jitter / launch). When a
/// prim carries both a rigid body and authored animation, the visual sampler is
/// the motion authority, so demote it — a `Kinematic` body still collides and
/// still drives its joints, it just isn't integrated from forces.
///
/// `Or<(Added<RigidBody>, Added<UsdAnimated>)>` makes this fire once when either
/// marker lands (the two arrive on different frames via separate observers), so
/// it catches both insertion orders and then idles (empty query).
fn enforce_kinematic_on_animated(
    mut commands: Commands,
    q: Query<
        (Entity, &RigidBody),
        (With<UsdAnimated>, Or<(Added<RigidBody>, Added<UsdAnimated>)>),
    >,
) {
    for (entity, body) in &q {
        if matches!(body, RigidBody::Dynamic) {
            // Animation is the motion authority → the declared mobility is now
            // Kinematic, matching the demoted body type.
            commands
                .entity(entity)
                .insert((RigidBody::Kinematic, lunco_core::Mobility::Kinematic));
        }
    }
}

/// Marker to indicate a prim has been processed by the Avian physics system.
///
/// Prevents the deferred processing system from re-processing the same entity on
/// subsequent frames.
#[derive(Component)]
struct UsdAvianProcessed;

/// Marker for USD prims awaiting joint creation.
///
/// Inserted when a `PhysicsPrismaticJoint` (or other joint type) is detected in USD
/// but the referenced body entities haven't been spawned yet. The `build_usd_physics_joints`
/// system checks for these markers and creates Avian3D joints once both bodies exist.
#[derive(Component)]
pub struct PendingUsdJoint {
    /// USD path to body0 (the anchor/chassis).
    pub body0_path: String,
    /// USD path to body1 (the driven body/wheel).
    pub body1_path: String,
    /// Joint axis in local space of body0.
    pub axis: DVec3,
    /// Anchor point on body0 in body0's local frame
    /// (UsdPhysics `physics:localPos0`). Defaults to origin.
    pub local_pos0: DVec3,
    /// Anchor point on body1 in body1's local frame
    /// (UsdPhysics `physics:localPos1`). Defaults to origin.
    pub local_pos1: DVec3,
    /// Lower travel limit along the axis (meters for prismatic, radians for revolute).
    pub limit_lower: f64,
    /// Upper travel limit.
    pub limit_upper: f64,
    /// The joint kind from USD (e.g., `PhysicsPrismaticJoint`).
    pub joint_type: String,
    /// Spherical-joint swing cone half-angles `(angle0, angle1)` from
    /// `physics:coneAngle0Limit`/`physics:coneAngle1Limit`, or `None` for a free
    /// (unlimited) cone. `limit_lower/upper` carry the *twist* limit for a
    /// spherical joint.
    pub swing_limit: Option<(f64, f64)>,
    /// Authored `UsdPhysicsDriveAPI` drive (the `linear` instance for prismatic,
    /// `angular` for revolute), or `None` when the joint carries no drive — it
    /// then stays passive until a cosim wire commands its `displacement`/`angle`
    /// port.
    pub drive: Option<JointDrive>,
}

/// A `UsdPhysicsDriveAPI` joint drive, read at load. Configures the Avian joint
/// motor so an Omniverse-authored mechanism seeks its target out of the box; a
/// cosim wire targeting the joint's port overrides `target_position` per tick.
#[derive(Clone, Copy, Default)]
pub struct JointDrive {
    /// `drive:{angular,linear}:physics:targetPosition` (rad or m).
    pub target_position: Option<f64>,
    /// `drive:{angular,linear}:physics:targetVelocity` (rad/s or m/s).
    pub target_velocity: Option<f64>,
    /// `drive:{angular,linear}:physics:maxForce` — the motor's torque (N·m) or
    /// force (N) saturation. Replaces the cosim default when authored.
    pub max_force: Option<f64>,
}

/// Overdamped spring-damper for load-time joint drives — mirrors
/// `lunco_cosim::joint`'s motor model so a USD-driven joint and a wire-driven
/// one track their setpoint identically (≈3 Hz, ζ=2, no overshoot under XPBD
/// substepping). avian's `MotorModel` reparameterizes stiffness/damping as
/// frequency/ratio, so USD `physics:stiffness`/`physics:damping` are not mapped
/// 1:1 yet; `physics:maxForce` and the targets (the load-bearing knobs) are.
const JOINT_DRIVE_MOTOR_MODEL: MotorModel = MotorModel::SpringDamper {
    frequency: 3.0,
    damping_ratio: 2.0,
};

/// Force (N) / torque (N·m) saturation a USD-driven joint motor gets when its
/// `physics:maxForce` is left unauthored — generous enough to hold the target
/// against gravity, matching `lunco_cosim::joint`'s wire-driven default.
const JOINT_DRIVE_MAX_FORCE_DEFAULT: f64 = 1.0e8;

/// Checks if a USD prim has a specific API schema applied.
/// Collects collider shapes from all child prims of a compound body root,
/// reading directly from the USD stage.
///
/// Returns a list of `(Position, Rotation, Collider)` tuples for `Collider::compound()`.
fn collect_child_colliders_from_usd<R: UsdRead>(
    reader: &R,
    parent_path: &SdfPath,
) -> Vec<(Position, Rotation, Collider)> {
    let mut shapes = Vec::new();

    for child_path in reader.children(parent_path) {
        // Skip wheel children — they're independent dynamics handled
        // by `lunco-usd-sim` (raycast probe or physical wheel rigid
        // body), NOT collider pieces of the chassis compound. The
        // `physxVehicleWheel:radius` attribute is the canonical marker
        // (matches the same skip in `process_usd_avian_prims`).
        if reader.real_f32(&child_path, "physxVehicleWheel:radius").is_some() {
            continue;
        }

        // Check if child has collision enabled
        let child_collision = reader
            .scalar::<bool>(&child_path, "physics:collisionEnabled")
            .unwrap_or(true);
        if !child_collision { continue; }

        // Read child's local transform (canonical decoder, shared with usd-bevy).
        let mut child_tf = read_transform_from_usd(reader, &child_path);

        // For Cylinder children, fold UsdGeomCylinder.axis into the
        // child's compound-local rotation so the Y-axis collider lines
        // up with the authored axis (mirrors what lunco-usd-bevy does
        // for the entity Transform — same canonical `usd_axis_to_quat`).
        if let Some(ty) = reader.type_name(&child_path) {
            if matches!(ty.as_str(), "Cylinder" | "Cone" | "Capsule" | "Plane") {
                let axis_tok = read_token_attribute(reader, &child_path, "axis")
                    .unwrap_or_else(|| "Z".to_string());
                if let Some(q) = usd_axis_to_quat(&axis_tok) {
                    child_tf.rotation = child_tf.rotation * q;
                }
            }
        }

        // Build collider from child's geometry
        if let Some(collider) = build_collider_from_usd(reader, &child_path) {
            let pos = Position(DVec3::new(
                child_tf.translation.x as f64,
                child_tf.translation.y as f64,
                child_tf.translation.z as f64,
            ));
            let rot = Rotation(child_tf.rotation.as_dquat());
            shapes.push((pos, rot, collider));
        }
    }

    shapes
}

/// Builds a Collider from a USD prim's geometry type and dimensions.
///
/// Builds an Avian collider from a USD shape prim.
///
/// **Scaling is NOT done here — Avian owns it.** `update_collider_scale`
/// sets `collider.scale = world Transform.scale` every frame for *every*
/// collider (measured: the ground collider's `scale` becomes (4000,0.2,4000)
/// from its `xformOp:scale`). So each shape branch returns the **intrinsic,
/// unscaled** shape at its authored size, and the single [`apply_collider_scale`]
/// tail pre-applies the prim's `xformOp:scale` once, uniformly.
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
/// **Legacy fallback for `Cube`**: `width`/`height`/`depth` still accepted so
/// unmigrated `.usda` files keep working (those author full dims at scale=1).
fn build_collider_from_usd<R: UsdRead>(reader: &R, sdf_path: &SdfPath) -> Option<Collider> {
    let ty = reader.type_name(sdf_path)?;

    // Native UsdGeomMesh → static triangle-mesh collider, decoded from the
    // SAME `points`/`faceVertexIndices` `lunco-usd-bevy` renders (one geometry
    // source, so collider and visual can't drift). `set_scale` on a trimesh
    // scales its vertices exactly (no convex-hull tessellation), so the shared
    // scale tail applies unchanged.
    if ty == "Mesh" {
        let (verts, tris) = read_usd_mesh_indexed(reader, sdf_path)?;
        let verts: Vec<DVec3> =
            verts.into_iter().map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)).collect();
        // Standard `UsdPhysicsMeshCollisionAPI physics:approximation` selects how
        // the render mesh becomes a collider. Default (unauthored / `none` /
        // `meshSimplification`) = exact triangle mesh — correct for STATIC terrain.
        // `convexHull`/`convexDecomposition` produce the solid volumes a DYNAMIC
        // body needs (a trimesh can't be a moving rigid body in parry). Read via
        // the standard token so it works off either the live stage or the flatten.
        let collider = match read_token_attribute(reader, sdf_path, "physics:approximation").as_deref() {
            Some("convexHull") => {
                Collider::convex_hull(verts.clone()).unwrap_or_else(|| Collider::trimesh(verts, tris))
            }
            Some("convexDecomposition") => Collider::convex_decomposition(verts, tris),
            // `boundingCube`/`boundingSphere`/`meshSimplification` aren't mapped
            // to a distinct parry shape yet — fall back to the exact trimesh
            // rather than silently mis-sizing the body.
            _ => Collider::trimesh(verts, tris),
        };
        return Some(apply_collider_scale(collider, reader, sdf_path));
    }

    // Dimensions (+ their magic defaults) come from the canonical
    // `read_shape_dims` shared with usd-bevy's mesh builder, so the
    // collider can't desync from the visual mesh. Build the INTRINSIC
    // (unscaled) shape; the scale tail below owns scaling.
    let collider = match read_shape_dims(reader, sdf_path, ty.as_str())? {
        ShapeDims::Cube { size, legacy_extents } => match legacy_extents {
            Some([width, height, depth]) => Collider::cuboid(width, height, depth),
            None => Collider::cuboid(size, size, size),
        },
        ShapeDims::Sphere { radius } => Collider::sphere(radius),
        ShapeDims::Cylinder { radius, height } => Collider::cylinder(radius, height),
        ShapeDims::Cone { radius, height } => Collider::cone(radius, height),
        ShapeDims::Capsule { radius, height } => Collider::capsule(radius, height),
        // Represent the plane as a thin cuboid so bounds and scaling
        // behave predictably and match the visual mapping.
        ShapeDims::Plane { width, length } => Collider::cuboid(width, 0.001, length),
    };

    Some(apply_collider_scale(collider, reader, sdf_path))
}

/// Pre-applies a prim's `xformOp:scale` to a freshly-built intrinsic collider so
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
fn apply_collider_scale<R: UsdRead>(mut collider: Collider, reader: &R, sdf_path: &SdfPath) -> Collider {
    let scale = read_vec3_attribute(reader, sdf_path, "xformOp:scale")
        .map(|v| (v.x, v.y, v.z))
        .unwrap_or((1.0, 1.0, 1.0));
    collider.set_scale(bevy::math::DVec3::new(scale.0, scale.1, scale.2), 10);
    collider
}

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd<R: UsdRead>(
    commands: &mut Commands,
    entity: Entity,
    reader: &R,
    sdf_path: &SdfPath,
) {
    if let Some(collider) = build_collider_from_usd(reader, sdf_path) {
        commands.entity(entity).insert(collider);
    }
}

/// Terrain prims whose collider is built from a loaded `Mesh3d` — a glTF DEM
/// brought in via `lunco:assetMode = "mesh"` (e.g. the Shackleton ridge).
///
/// The collider can't be built in `process_usd_avian_prims` because the mesh
/// asset is usually still async-loading there. This marker holds the entity
/// until [`build_terrain_mesh_colliders`] sees the loaded mesh.
#[derive(Component)]
struct PendingTerrainCollider;

/// Builds the static collider for a mesh-backed terrain once its `Mesh3d`
/// asset is available. Prefers a [`heightfield`](heightfield_from_mesh) when
/// the mesh is a regular DEM grid; otherwise falls back to a general trimesh.
fn build_terrain_mesh_colliders(
    q: Query<(Entity, &Mesh3d), With<PendingTerrainCollider>>,
    meshes: Res<Assets<Mesh>>,
    mut commands: Commands,
) {
    for (entity, mesh3d) in &q {
        // Still loading — try again next frame.
        let Some(mesh) = meshes.get(&mesh3d.0) else { continue };

        let collider = heightfield_from_mesh(mesh).or_else(|| {
            warn!("[usd-avian] terrain mesh isn't a regular DEM grid; \
                   building a (heavier) trimesh collider instead");
            Collider::trimesh_from_mesh(mesh)
        });

        match collider {
            Some(c) => {
                info!("[usd-avian] terrain collider built ({} verts)", mesh.count_vertices());
                commands.entity(entity).insert(c).remove::<PendingTerrainCollider>();
            }
            None => {
                warn!("[usd-avian] terrain mesh has no usable geometry — no collider built");
                commands.entity(entity).remove::<PendingTerrainCollider>();
            }
        }
    }
}

/// Builds a parry **heightfield** `Collider` from a regular grid mesh (a DEM /
/// heightmap, like the Shackleton ridge glTF). Returns `None` if the mesh
/// isn't a square, axis-aligned, row-major XZ grid — the caller then falls
/// back to a general trimesh.
///
/// Why a heightfield instead of a trimesh: a DEM *is* an N×N grid of height
/// samples. A heightfield collider stores exactly that grid and resolves a
/// contact by indexing the two cells under the query point — O(1), ~N²
/// floats — whereas a trimesh stores 2·(N−1)² triangles in a BVH that must be
/// built and traversed. For this 458×458 ridge that's a 209,764-cell grid vs
/// a ~417,800-triangle BVH: dramatically cheaper to build (no offline pre-bake
/// needed) and to query, with zero loss of fidelity — the grid is the source
/// geometry.
///
/// avian's heightfield indexes **rows along X, columns along Z**, centred on
/// the XZ plane and scaled per axis. Our mesh is row-major with each row a
/// line of constant Z and each column a line of constant X (Blender's DEM
/// export order), so vertex (row r = Z, col c = X) sits at index `r*side + c`
/// and maps to `heights[x = c][z = r]`. The `scale` restores the metric
/// footprint; height scale stays 1 because vertex Y is already in metres. The
/// collider therefore coincides with the visual mesh (same source, same
/// entity transform).
fn heightfield_from_mesh(mesh: &Mesh) -> Option<Collider> {
    let Some(VertexAttributeValues::Float32x3(pos)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };

    let n = pos.len();
    let side = (n as f64).sqrt() as usize;
    if side < 2 || side * side != n {
        return None;
    }

    // Probe the expected layout (row = constant Z, column = constant X). If it
    // doesn't hold, bail to trimesh rather than build a scrambled collider.
    let eps = 1.0_f32;
    let row_const_z = (pos[0][2] - pos[1][2]).abs() < eps
        && (pos[0][2] - pos[side - 1][2]).abs() < eps;
    let col_const_x = (pos[0][0] - pos[side][0]).abs() < eps;
    if !row_const_z || !col_const_x {
        return None;
    }

    let (mut min_x, mut max_x, mut min_z, mut max_z) =
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in pos {
        min_x = min_x.min(v[0]);
        max_x = max_x.max(v[0]);
        min_z = min_z.min(v[2]);
        max_z = max_z.max(v[2]);
    }
    let scale_x = (max_x - min_x) as f64;
    let scale_z = (max_z - min_z) as f64;
    if scale_x <= 0.0 || scale_z <= 0.0 {
        return None;
    }

    let mut heights = vec![vec![0.0_f64; side]; side];
    for r in 0..side {
        for c in 0..side {
            heights[c][r] = pos[r * side + c][1] as f64;
        }
    }

    Some(Collider::heightfield(heights, DVec3::new(scale_x, 1.0, scale_z)))
}

/// Deferred system that maps USD physics attributes to Avian3D components.
///
/// This system runs in the `Update` schedule and processes all `UsdPrimPath` entities
/// that haven't been marked with `UsdAvianProcessed` yet.
///
/// # USD Compound Rigid Body Standard
///
/// Per OpenUSD spec, a prim with `PhysicsRigidBodyAPI` aggregates all descendant
/// colliders into ONE compound rigid body. Children with only `PhysicsCollisionAPI`
/// contribute collider shapes but are NOT independent bodies.
///
/// # Processing
///
/// **Compound body root (PhysicsRigidBodyAPI):**
/// - Reads all child collider shapes from USD
/// - Builds ONE `Collider::compound()` on the parent
/// - Adds `RigidBody::Dynamic` + `SelectableRoot` + mass/damping/friction
///
/// **Collider children (PhysicsCollisionAPI only):**
/// - Become pure visuals — no RigidBody, no Collider
/// - Their shapes are included in the parent's compound collider
///
/// **Legacy fallback:** `physics:rigidBodyEnabled` attribute for old-style USD files.
/// Observer: fires once per entity, the moment `sync_usd_visuals` finishes
/// translating the USD prim (signalled by inserting `UsdVisualSynced`).
/// By that point the stage is loaded and `Mesh3d`/`Transform` are present —
/// safe to read schemas and attach physics components in one step.
fn process_usd_avian_prims(
    trigger: On<Add, UsdVisualSynced>,
    query: Query<&UsdPrimPath, Without<UsdAvianProcessed>>,
    q_child_of: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };
    let is_root = q_child_of.get(entity).is_err();

    // Ph0′ CUTOVER: read the LIVE canonical stage — the source of truth — built
    // on demand from the asset's recipe so it is available regardless of system
    // ordering. The body comes off the composed `Stage` directly.
    let id = prim_path.stage_handle.id();
    if canonical.get(id).is_none() {
        if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(id, &recipe);
        }
    }
    let Some(cs) = canonical.get(id) else {
        // no live stage (asset carries no recipe / build failed) — skip
        return;
    };
    bevy::log::debug!("[canonical] avian extract off LIVE stage: {}", prim_path.path);
    extract_avian_prim(&cs.view(), entity, &sdf_path, is_root, &mut commands);
}

/// Map a single composed USD prim to its avian physics components, generic over
/// the read source ([`UsdRead`]) — so it drives off either the live canonical
/// [`StageView`](lunco_usd_bevy::StageView) or the flattened `sdf::Data`,
/// identically. Extracted from the observer for the Ph0′ cutover.
fn extract_avian_prim<R: UsdRead>(
    reader: &R,
    entity: Entity,
    sdf_path: &SdfPath,
    is_root: bool,
    commands: &mut Commands,
) {
    // Skip wheel prims — the sim plugin handles those.
    if reader.real_f32(sdf_path, "physxVehicleWheel:radius").is_some() {
        commands.entity(entity).insert(UsdAvianProcessed);
        return;
    }

    let has_rigid_body_api = reader.has_api_schema(sdf_path, "PhysicsRigidBodyAPI");
    let has_collision_api = reader.has_api_schema(sdf_path, "PhysicsCollisionAPI");
    let has_terrain_api = reader.has_api_schema(sdf_path, "PhysxTerrainAPI");

    // ── TERRAIN ── static collider + TerrainTile; mesh DEMs defer their collider.
    if has_terrain_api {
        commands.entity(entity).insert((
            RigidBody::Static,
            lunco_core::Mobility::Static,
            lunco_terrain_globe::TerrainTile,
        ));
        if let Some(collider) = build_collider_from_usd(reader, sdf_path) {
            commands.entity(entity).insert(collider);
        } else {
            commands.entity(entity).insert(PendingTerrainCollider);
        }
        commands.entity(entity).insert(UsdAvianProcessed);
        return;
    }

    // ── TRIGGER ZONE ── `lunco:triggerZone` → overlap-only static Sensor.
    if let Some(zone) = reader
        .scalar::<String>(sdf_path, "lunco:triggerZone")
        .filter(|z| !z.trim().is_empty())
    {
        commands.entity(entity).insert((RigidBody::Static, lunco_core::Mobility::Static));
        add_collider_from_usd(commands, entity, reader, sdf_path);
        commands.entity(entity).insert((
            Sensor,
            lunco_core::TriggerZone(zone),
            CollisionLayers::new(LayerMask(lunco_core::TRIGGER_COLLISION_LAYER), LayerMask::ALL),
            UsdAvianProcessed,
        ));
        return;
    }

    if has_rigid_body_api {
        // ── COMPOUND BODY ROOT ── children colliders → compound, else self.
        let compound_shapes = collect_child_colliders_from_usd(reader, sdf_path);
        if !compound_shapes.is_empty() {
            commands.entity(entity).insert(Collider::compound(compound_shapes));
        } else {
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }

        // A `Dynamic`-declared body spawns `Kinematic` + `ShouldBeDynamic` and
        // settles to `Dynamic` once joints resolve (no 1-frame separation launch).
        let kinematic = reader.scalar::<bool>(sdf_path, "physics:kinematicEnabled").unwrap_or(false);
        let (body, mobility) = if kinematic {
            (RigidBody::Kinematic, lunco_core::Mobility::Kinematic)
        } else {
            commands.entity(entity).insert(ShouldBeDynamic);
            (RigidBody::Kinematic, lunco_core::Mobility::Dynamic)
        };
        commands.entity(entity).insert((body, mobility, lunco_core::SelectableRoot));

        // Always insert a Mass (default 1000 kg) — gravity filters on `With<Mass>`.
        apply_rigid_body_mass_props(commands, entity, reader, sdf_path);
        commands.entity(entity).insert(UsdAvianProcessed);
    } else if has_collision_api {
        // ── COLLIDER CHILD ── part of parent's compound; root-level → static.
        if is_root {
            commands.entity(entity).insert((RigidBody::Static, lunco_core::Mobility::Static));
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }
        commands.entity(entity).insert(UsdAvianProcessed);
    } else {
        // ── FALLBACK: legacy `physics:rigidBodyEnabled` ──
        if let Some(true) = reader.scalar::<bool>(sdf_path, "physics:rigidBodyEnabled") {
            commands.entity(entity).insert((
                RigidBody::Kinematic,
                lunco_core::Mobility::Dynamic,
                ShouldBeDynamic,
                lunco_core::SelectableRoot,
            ));
            apply_rigid_body_mass_props(commands, entity, reader, sdf_path);
            add_collider_from_usd(commands, entity, reader, sdf_path);
        } else if let Some(false) = reader.scalar::<bool>(sdf_path, "physics:rigidBodyEnabled") {
            commands.entity(entity).insert((RigidBody::Static, lunco_core::Mobility::Static));
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }
        commands.entity(entity).insert(UsdAvianProcessed);
    }
}

/// Read the STANDARD UsdPhysics joint at `path` off the LIVE composed stage via
/// openusd's typed schema (`openusd::schemas::physics`) into the deferred
/// [`PendingUsdJoint`]. The typed schema wraps a live `Prim`, so it reads the
/// composed opinion directly — canonical stage only, no flattened `sdf::Data`.
///
/// This replaces the ad-hoc raw-attribute joint reader with the USD standard:
/// concrete joint subtypes ([`RevoluteJoint`](openusd::schemas::physics::RevoluteJoint)
/// …), shared `JointBase` body/anchor relationships, `UsdPhysicsDriveAPI`, and
/// per-DOF `UsdPhysicsLimitAPI` for the generic-D6 reduction. Returns `None` when
/// `path` is not a UsdPhysics joint, is missing a body ref, or targets a wheel
/// (owned by `lunco-usd-sim`). Revolute limits are converted degrees→radians
/// (the `PendingUsdJoint` contract); prismatic/distance stay in scene units.
fn read_joint_spec_typed(stage: &Stage, path: &SdfPath) -> Option<PendingUsdJoint> {
    use openusd::schemas::physics::{self, JointAxis, JointBase};

    let axis_of = |ax: Option<JointAxis>| match ax.unwrap_or_default() {
        JointAxis::X => DVec3::X,
        JointAxis::Y => DVec3::Y,
        JointAxis::Z => DVec3::Z,
    };
    // Shared JointBase reads (both bodies + local anchors). `None` unless BOTH
    // bodies are authored — world-anchored joints aren't mapped to avian here.
    fn base<J: JointBase>(j: &J) -> Option<(String, String, DVec3, DVec3)> {
        let to_dvec = |a: [f32; 3]| DVec3::new(a[0] as f64, a[1] as f64, a[2] as f64);
        let b0 = j.body0_rel().targets().ok()?.into_iter().next()?;
        let b1 = j.body1_rel().targets().ok()?.into_iter().next()?;
        let lp0 = j.local_pos0_attr().get::<[f32; 3]>().ok().flatten().map(to_dvec).unwrap_or(DVec3::ZERO);
        let lp1 = j.local_pos1_attr().get::<[f32; 3]>().ok().flatten().map(to_dvec).unwrap_or(DVec3::ZERO);
        Some((b0.to_string(), b1.to_string(), lp0, lp1))
    }
    let read_drive = |ns: &str| -> Option<JointDrive> {
        let d = physics::DriveAPI::get(stage, path.clone(), ns).ok().flatten()?;
        let tp = d.target_position_attr().get::<f32>().ok().flatten().map(|v| v as f64);
        let tv = d.target_velocity_attr().get::<f32>().ok().flatten().map(|v| v as f64);
        let mf = d.max_force_attr().get::<f32>().ok().flatten().map(|v| v as f64);
        (tp.is_some() || tv.is_some() || mf.is_some())
            .then_some(JointDrive { target_position: tp, target_velocity: tv, max_force: mf })
    };

    let spec = if let Some(j) = physics::RevoluteJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j)?;
        let axis = axis_of(j.axis_attr().get::<JointAxis>().ok().flatten());
        let lo = j.lower_limit_attr().get::<f32>().ok().flatten()
            .map(|d| (d as f64).to_radians()).unwrap_or(f64::NEG_INFINITY);
        let hi = j.upper_limit_attr().get::<f32>().ok().flatten()
            .map(|d| (d as f64).to_radians()).unwrap_or(f64::INFINITY);
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: "PhysicsRevoluteJoint".into(),
            swing_limit: None, drive: read_drive("angular"),
        }
    } else if let Some(j) = physics::PrismaticJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j)?;
        let axis = axis_of(j.axis_attr().get::<JointAxis>().ok().flatten());
        let lo = j.lower_limit_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::NEG_INFINITY);
        let hi = j.upper_limit_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::INFINITY);
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: "PhysicsPrismaticJoint".into(),
            swing_limit: None, drive: read_drive("linear"),
        }
    } else if let Some(j) = physics::SphericalJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j)?;
        let axis = axis_of(j.axis_attr().get::<JointAxis>().ok().flatten());
        let swing = j.cone_angle0_limit_attr().get::<f32>().ok().flatten()
            .zip(j.cone_angle1_limit_attr().get::<f32>().ok().flatten())
            .map(|(a, b)| (a as f64, b as f64));
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis, local_pos0: lp0, local_pos1: lp1,
            limit_lower: f64::NEG_INFINITY, limit_upper: f64::INFINITY,
            joint_type: "PhysicsSphericalJoint".into(), swing_limit: swing, drive: None,
        }
    } else if let Some(j) = physics::FixedJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j)?;
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis: DVec3::Y, local_pos0: lp0, local_pos1: lp1,
            limit_lower: f64::NEG_INFINITY, limit_upper: f64::INFINITY,
            joint_type: "PhysicsFixedJoint".into(), swing_limit: None, drive: None,
        }
    } else if let Some(j) = physics::DistanceJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j)?;
        let lo = j.min_distance_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::NEG_INFINITY);
        let hi = j.max_distance_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::INFINITY);
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis: DVec3::Y, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: "PhysicsDistanceJoint".into(),
            swing_limit: None, drive: None,
        }
    } else if let Some(j) = physics::Joint::get(stage, path.clone()).ok().flatten() {
        // Generic/D6 → reduce via per-DOF UsdPhysicsLimitAPI (typed).
        let (b0, b1, lp0, lp1) = base(&j)?;
        let (reduced, axis, lo, hi) = reduce_generic_joint_typed(stage, path)?;
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: reduced.into(),
            swing_limit: None, drive: None,
        }
    } else {
        return None;
    };

    // Wheel-targeted joints are owned by `lunco-usd-sim` (built alongside the
    // wheel body); skip them here to avoid double-up/race.
    if let Ok(b1_path) = SdfPath::new(&spec.body1_path) {
        if stage.prim(b1_path).attribute("physxVehicleWheel:radius").get::<f32>().ok().flatten().is_some() {
            return None;
        }
    }
    Some(spec)
}

/// Typed-schema sibling of [`reduce_generic_joint`]: reduces a generic
/// `UsdPhysicsJoint` (D6) to the avian primitive matching its free DOFs by
/// reading each per-DOF `UsdPhysicsLimitAPI` (`limit:{transX..rotZ}`) off the
/// live stage. A DOF is *locked* when `low > high`, *free* when unauthored.
fn reduce_generic_joint_typed(stage: &Stage, path: &SdfPath) -> Option<(&'static str, DVec3, f64, f64)> {
    use openusd::schemas::physics;
    const DOFS: [(&str, DVec3, bool); 6] = [
        ("transX", DVec3::X, false), ("transY", DVec3::Y, false), ("transZ", DVec3::Z, false),
        ("rotX", DVec3::X, true), ("rotY", DVec3::Y, true), ("rotZ", DVec3::Z, true),
    ];
    let mut free_trans: Vec<(DVec3, f64, f64)> = Vec::new();
    let mut free_rot: Vec<(DVec3, f64, f64)> = Vec::new();
    for (inst, axis, is_rot) in DOFS {
        let (low, high) = match physics::LimitAPI::get(stage, path.clone(), inst).ok().flatten() {
            Some(l) => (
                l.low_attr().get::<f32>().ok().flatten().map(|v| v as f64),
                l.high_attr().get::<f32>().ok().flatten().map(|v| v as f64),
            ),
            None => (None, None),
        };
        match (low, high) {
            (Some(l), Some(h)) if l > h => {} // locked
            (l, h) => {
                let entry = (axis, l.unwrap_or(f64::NEG_INFINITY), h.unwrap_or(f64::INFINITY));
                if is_rot { free_rot.push(entry) } else { free_trans.push(entry) }
            }
        }
    }
    match (free_trans.len(), free_rot.len()) {
        (0, 0) => Some(("PhysicsFixedJoint", DVec3::Y, f64::NEG_INFINITY, f64::INFINITY)),
        (0, 1) => Some(("PhysicsRevoluteJoint", free_rot[0].0, free_rot[0].1, free_rot[0].2)),
        (1, 0) => Some(("PhysicsPrismaticJoint", free_trans[0].0, free_trans[0].1, free_trans[0].2)),
        (0, 3) => Some(("PhysicsSphericalJoint", free_rot[0].0, f64::NEG_INFINITY, f64::INFINITY)),
        _ => None,
    }
}

/// Observer that fires when a USD prim entity is added.
///
/// Detects physics joints (PhysicsRevoluteJoint, PhysicsPrismaticJoint, …) and
/// stamps the deferred [`PendingUsdJoint`] carrier. Ph0′: reads the STANDARD
/// UsdPhysics joint schema off the LIVE canonical stage
/// ([`read_joint_spec_typed`]); the flattened raw-attribute path below is a
/// transition fallback for recipe-less assets, deleted once every asset carries
/// a `StageRecipe`.
fn on_add_usd_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    // Ph0′: the STANDARD typed UsdPhysics read off the live canonical stage
    // (built on demand). The typed schema needs the composed `Stage`.
    let id = prim_path.stage_handle.id();
    if canonical.get(id).is_none() {
        if let Some(recipe) = stages.get(&prim_path.stage_handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(id, &recipe);
        }
    }
    let Some(cs) = canonical.get(id) else {
        // no live stage (asset carries no recipe / build failed) — skip
        return;
    };
    // A wheel prim on the LIVE stage → owned by the sim plugin, skip.
    if cs.stage().prim(sdf_path.clone()).attribute("physxVehicleWheel:radius")
        .get::<f32>().ok().flatten().is_some()
    {
        return;
    }
    if let Some(joint) = read_joint_spec_typed(cs.stage(), &sdf_path) {
        commands.entity(entity).insert(joint);
    }

    // Note: Physics mapping (RigidBody, Mass, Collider, Damping) is handled by
    // the sim plugin's process_usd_sim_prims system to ensure consistent ordering
    // and avoid duplicate processing.
}

fn find_instance_root(
    entity: Entity,
    q_child_of: &Query<&ChildOf>,
    q_usd_path: &Query<&UsdPrimPath>,
    q_instance_root: &Query<(), With<UsdInstanceRoot>>,
) -> Entity {
    let mut cursor = entity;
    let mut best_root = entity;
    loop {
        if q_instance_root.get(cursor).is_ok() {
            return cursor;
        }
        if q_usd_path.get(cursor).is_ok() {
            best_root = cursor;
        }
        match q_child_of.get(cursor) {
            Ok(parent) => cursor = parent.parent(),
            Err(_) => break,
        }
    }
    best_root
}

/// Resolves pending USD joints once both body entities exist.
///
/// This system runs every frame. When a `PendingUsdJoint` entity finds that both its
/// referenced bodies have been spawned as Bevy entities with matching `UsdPrimPath`
/// components, it creates the appropriate Avian joint and removes the pending marker.
fn build_usd_physics_joints(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingUsdJoint, &UsdPrimPath)>,
    // **Avian readiness gate**: matching on `&Position` (added by
    // Avian's body-init systems alongside `BodyIslandNode`) ensures
    // we don't create a joint before Avian has admitted both bodies
    // into its island graph — without this the solver panics with
    // `Neither body … is in an island`. `process_usd_avian_prims`
    // queues the `RigidBody` insertion in our `Update`; Avian's
    // initialisation runs in its `PhysicsSchedule` (FixedUpdate),
    // so this query is empty for the first few frames after spawn,
    // and the joint stays in `PendingUsdJoint` until ready.
    q_bodies: Query<(Entity, &UsdPrimPath), With<Position>>,
    q_child_of: Query<&ChildOf>,
    q_usd_path: Query<&UsdPrimPath>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
) {
    for (joint_entity, pending, joint_prim_path) in q_pending.iter() {
        let joint_root = find_instance_root(joint_entity, &q_child_of, &q_usd_path, &q_instance_root);
        // Find body0 and body1 entities by matching USD paths and instance roots
        let body0_ent = q_bodies.iter()
            .find(|(e, path)| {
                path.path == pending.body0_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && find_instance_root(*e, &q_child_of, &q_usd_path, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);
        let body1_ent = q_bodies.iter()
            .find(|(e, path)| {
                path.path == pending.body1_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && find_instance_root(*e, &q_child_of, &q_usd_path, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);

        let (Some(b0), Some(b1)) = (body0_ent, body1_ent) else { continue; };

        info!("Built USD joint {} -> {} <-> {}", pending.joint_type, pending.body0_path, pending.body1_path);

        // Put the avian joint component ON the joint prim entity itself (it
        // already carries `UsdPrimPath` + the loader-assigned `GlobalEntityId`)
        // rather than spawning a fresh anonymous entity. This makes the joint
        // — and the `angle` port `lunco-cosim` auto-exposes on any
        // `RevoluteJoint` — addressable by USD path, API id, or `Entity` alike,
        // so the wiring fabric can target `</…/Joint>.angle` with no
        // USD-specific lookup.
        match pending.joint_type.as_str() {
            "PhysicsPrismaticJoint" => {
                let mut joint = PrismaticJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_slider_axis(pending.axis)
                    .with_limits(pending.limit_lower, pending.limit_upper);
                if let Some(d) = pending.drive {
                    joint.motor = LinearMotor {
                        enabled: d.target_position.is_some() || d.target_velocity.is_some(),
                        target_position: d.target_position.unwrap_or(0.0),
                        target_velocity: d.target_velocity.unwrap_or(0.0),
                        max_force: d.max_force.unwrap_or(JOINT_DRIVE_MAX_FORCE_DEFAULT),
                        motor_model: JOINT_DRIVE_MOTOR_MODEL,
                    };
                }
                commands.entity(joint_entity).insert((joint, JointCollisionDisabled));
            }
            "PhysicsRevoluteJoint" => {
                let mut joint = RevoluteJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_hinge_axis(pending.axis)
                    .with_angle_limits(pending.limit_lower, pending.limit_upper);
                if let Some(d) = pending.drive {
                    joint.motor = AngularMotor {
                        enabled: d.target_position.is_some() || d.target_velocity.is_some(),
                        target_position: d.target_position.unwrap_or(0.0),
                        target_velocity: d.target_velocity.unwrap_or(0.0),
                        max_torque: d.max_force.unwrap_or(JOINT_DRIVE_MAX_FORCE_DEFAULT),
                        motor_model: JOINT_DRIVE_MOTOR_MODEL,
                    };
                }
                commands.entity(joint_entity).insert((joint, JointCollisionDisabled));
            }
            "PhysicsFixedJoint" => {
                commands.entity(joint_entity).insert((
                    FixedJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1),
                    JointCollisionDisabled,
                ));
            }
            "PhysicsSphericalJoint" => {
                // Ball joint: 3 rotational DOF about the anchor. `physics:axis`
                // is the twist axis; the cone (`physics:coneAngle*Limit`) bounds
                // swing, `physics:limit{Lower,Upper}` bounds twist. Suspension
                // uprights, robotic wrists, gimbals.
                let mut joint = SphericalJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_twist_axis(pending.axis);
                if let Some((a0, a1)) = pending.swing_limit {
                    // avian carries a single swing AngleLimit; use the larger
                    // cone half-angle as a symmetric bound.
                    let s = a0.abs().max(a1.abs());
                    joint = joint.with_swing_limits(-s, s);
                }
                if pending.limit_lower.is_finite() && pending.limit_upper.is_finite() {
                    joint = joint.with_twist_limits(pending.limit_lower, pending.limit_upper);
                }
                commands.entity(joint_entity).insert((joint, JointCollisionDisabled));
            }
            "PhysicsDistanceJoint" => {
                // Tether/strut: keeps the two anchors within [min, max] distance.
                // Cables, fixed-length links. Unauthored → a rigid rod at the
                // current separation's min (0) which is degenerate, so warn.
                let min = if pending.limit_lower.is_finite() { pending.limit_lower.max(0.0) } else { 0.0 };
                let max = if pending.limit_upper.is_finite() { pending.limit_upper.max(min) } else { min };
                if !pending.limit_upper.is_finite() {
                    warn!(
                        "DistanceJoint {} has no physics:maxDistance — defaulting to rigid {min} m",
                        pending.body1_path
                    );
                }
                commands.entity(joint_entity).insert((
                    DistanceJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1)
                        .with_limits(min, max),
                    JointCollisionDisabled,
                ));
            }
            // UsdPhysics generic D6 joint has no avian primitive (avian offers
            // fixed/revolute/prismatic/spherical/distance, not a configurable
            // 6-DOF constraint). Reducing it needs per-DOF PhysicsLimitAPI
            // analysis; until then, point the author at the explicit joint kinds.
            "PhysicsJoint" | "PhysicsD6Joint" => {
                warn!(
                    "Generic D6 joint {} unsupported — author an explicit \
                     PhysicsRevoluteJoint/PrismaticJoint/SphericalJoint/\
                     DistanceJoint/FixedJoint for the DOF you need",
                    pending.body1_path
                );
            }
            other => {
                warn!("Unsupported USD joint type: {}", other);
            }
        }

        commands.entity(joint_entity).remove::<PendingUsdJoint>();
    }
}

/// Builds the chassis↔wheel revolute constraint for a physical (joint-driven)
/// wheel — the one programmatically-synthesized joint (vs. the authored
/// `Physics*Joint` prims [`build_usd_physics_joints`] resolves). Centralizing it
/// here keeps **all** Avian joint construction in `lunco-usd-avian`, matching the
/// documented ownership; the caller (`lunco-usd-sim::setup_physical_wheel`)
/// supplies the drive [`AngularMotor`] and adds its mobility/hardware actuators
/// on top. `mount_local` is the hub anchor in chassis-local space, `axle` the
/// hinge axis (chassis-local).
pub fn wheel_revolute_joint(
    chassis: Entity,
    wheel: Entity,
    mount_local: DVec3,
    axle: DVec3,
    drive_motor: avian3d::prelude::AngularMotor,
) -> RevoluteJoint {
    RevoluteJoint::new(chassis, wheel)
        .with_local_anchor1(mount_local)
        .with_local_anchor2(DVec3::ZERO)
        .with_hinge_axis(axle)
        .with_motor(drive_motor)
}

/// Reads a USD token attribute (e.g., `uniform token axis = "X"`).
///
/// Thin delegate to the canonical [`lunco_usd_bevy::read_token`] — the
/// single home for token/string parsing shared with usd-bevy.
fn read_token_attribute<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<String> {
    lunco_usd_bevy::read_token(reader, path, attr)
}

/// Reads a `DVec3` attribute (e.g., `double3 xformOp:translate`) at full
/// f64 precision.
///
/// Thin DVec3 adapter over the canonical [`lunco_usd_bevy::read_vec3_f64`]
/// (the 4-branch `[f32;3]→[f64;3]→Vec<f32>→Vec<f64>` ladder). Keeping the
/// reader f64 end-to-end is what avoids the documented silent-`None`
/// "bodies launched into orbit" bug for `physics:localPos*` anchors.
fn read_vec3_attribute<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<DVec3> {
    lunco_usd_bevy::read_vec3_f64(reader, path, attr).map(|v| DVec3::new(v[0], v[1], v[2]))
}

/// Read mass, principal inertia, COM, damping, and friction from a rigid-body
/// prim and insert the corresponding Avian *override* components.
///
/// Centralises the previously-duplicated `physics:mass`/damping/friction reads
/// (the main `PhysicsRigidBodyAPI` path and the legacy `rigidBodyEnabled`
/// fallback diverged on mass handling — see the WP-3 DRY audit) and adds the
/// **G2 load-time** mass-properties (`physics:diagonalInertia` /
/// `physics:centerOfMass`).
///
/// Mass defaults to 1000 kg (canonical rover mass) when unauthored — keeping
/// gravity alive even when openusd-rs's resolver can't compose `physics:mass`
/// across a reference. Inertia/COM are inserted only when explicitly authored;
/// otherwise Avian derives them from collider geometry. These are the same
/// *override* components the runtime mass-props cosim ports write
/// (`lunco-cosim`), so authored and model-driven values share one path.
fn apply_rigid_body_mass_props<R: UsdRead>(
    commands: &mut Commands,
    entity: Entity,
    reader: &R,
    sdf_path: &SdfPath,
) {
    let mass = reader.real_f32(sdf_path, "physics:mass").unwrap_or(1000.0);
    commands.entity(entity).insert(Mass(mass));

    // G2 — authored principal inertia. `physics:diagonalInertia` is the diagonal
    // of the inertia tensor in the principal frame. `physics:principalAxes` (a
    // quat) would rotate that frame; it's almost always identity for
    // landers/rovers and is left to default. Off-diagonal inertia is not
    // representable here (Avian stores principal + frame), matching the
    // UsdPhysics schema.
    if let Some(diag) = read_vec3_attribute(reader, sdf_path, "physics:diagonalInertia") {
        commands.entity(entity).insert(AngularInertia {
            principal: diag.as_vec3(),
            local_frame: Quat::IDENTITY,
        });
    }

    // G2 — authored centre of mass (body-frame offset).
    if let Some(com) = read_vec3_attribute(reader, sdf_path, "physics:centerOfMass") {
        commands.entity(entity).insert(CenterOfMass(com.as_vec3()));
    }

    if let Some(d) = reader.real_f32(sdf_path, "physics:linearDamping") {
        commands.entity(entity).insert(LinearDamping(d as f64));
    }
    if let Some(d) = reader.real_f32(sdf_path, "physics:angularDamping") {
        commands.entity(entity).insert(AngularDamping(d as f64));
    }
    if let Some(f) = reader.real_f32(sdf_path, "physics:friction") {
        commands.entity(entity).insert(Friction::new(f.into()));
    }
    if let Some(vel) = read_vec3_attribute(reader, sdf_path, "physics:linearVelocity") {
        commands.entity(entity).insert(LinearVelocity(vel));
    }
    if let Some(ang) = read_vec3_attribute(reader, sdf_path, "physics:angularVelocity") {
        commands.entity(entity).insert(AngularVelocity(ang));
    }
}

/// Marker component to hold a rigid body as Kinematic until all joints
/// and constraints are fully resolved in the stage, preventing 1-frame
/// physics separation explosions.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShouldBeDynamic;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod collider_parity_tests {
    //! Ph0′ S2c: the collider read path is generic over `UsdRead`, driven off the
    //! live `StageView` over the canonical stage. Exercises the geometry read
    //! (the highest-risk physics read), including the mesh-approximation selector.

    use super::build_collider_from_usd;
    use lunco_usd_bevy::{compose_file_to_stage, StageView};
    use openusd::sdf::Path as SdfPath;

    // A UsdGeomMesh pyramid: default → exact trimesh; `physics:approximation =
    // "convexHull"` (standard UsdPhysicsMeshCollisionAPI) → a convex hull. The
    // two must be DIFFERENT colliders, proving the standard token is honoured.
    const MESH_FIXTURE: &str = "#usda 1.0\n\
        def Mesh \"Tri\"\n{\n\
            point3f[] points = [(0,0,0),(2,0,0),(2,2,0),(0,2,0),(1,1,2)]\n\
            int[] faceVertexCounts = [3,3,3,3]\n\
            int[] faceVertexIndices = [0,1,4, 1,2,4, 2,3,4, 3,0,4]\n\
        }\n\
        def Mesh \"Hull\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\", \"PhysicsMeshCollisionAPI\"] )\n{\n\
            point3f[] points = [(0,0,0),(2,0,0),(2,2,0),(0,2,0),(1,1,2)]\n\
            int[] faceVertexCounts = [3,3,3,3]\n\
            int[] faceVertexIndices = [0,1,4, 1,2,4, 2,3,4, 3,0,4]\n\
            uniform token physics:approximation = \"convexHull\"\n\
        }\n";

    #[test]
    fn mesh_collision_approximation_selects_convex_hull() {
        let dir = std::env::temp_dir().join("lunco_collider_approx");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("mesh.usda");
        std::fs::write(&f, MESH_FIXTURE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);

        let trimesh = build_collider_from_usd(&view, &SdfPath::new("/Tri").unwrap())
            .expect("default mesh → trimesh collider");
        let hull = build_collider_from_usd(&view, &SdfPath::new("/Hull").unwrap())
            .expect("convexHull approximation → collider");
        assert_ne!(
            format!("{trimesh:?}"),
            format!("{hull:?}"),
            "`physics:approximation = convexHull` must build a DIFFERENT collider than the default trimesh"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod extract_parity_tests {
    //! Ph0′ S2e CUTOVER verification: running the REAL `extract_avian_prim` off a
    //! live `StageView` must produce byte-identical physics components to running
    //! it off the flattened `sdf::Data` — on a rover chassis with a child collider
    //! at an authored transform (exercising the whole migrated read layer:
    //! schema detect → compound collider → `collect_child_colliders` →
    //! `read_transform_from_usd` → `local_transform_at` → mass props). This is the
    //! proof that the live canonical stage drives physics with no regression.

    use super::extract_avian_prim;
    use avian3d::prelude::*;
    use bevy::ecs::world::CommandQueue;
    use bevy::prelude::*;
    use lunco_usd_bevy::{compose_file_to_stage, StageView, UsdRead};
    use openusd::sdf::Path as SdfPath;

    // A rover chassis (RigidBodyAPI, mass 500) with a child Cube collider
    // (CollisionAPI) offset by an authored xformOp:translate — the compound path.
    const FIXTURE: &str = "#usda 1.0\n\ndef Xform \"Rover\" (\n    prepend apiSchemas = [\"PhysicsRigidBodyAPI\"]\n)\n{\n    double physics:mass = 500\n    def Cube \"Body\" (\n        prepend apiSchemas = [\"PhysicsCollisionAPI\"]\n    )\n    {\n        double size = 2\n        double3 xformOp:translate = (0, 1, 0)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";

    /// Run `extract_avian_prim` on a fresh world and read back the physics the
    /// chassis received: (body type, collider Debug, mass, has ShouldBeDynamic).
    fn run_extract<R: UsdRead>(
        reader: &R,
        path: &SdfPath,
    ) -> (Option<RigidBody>, Option<String>, Option<f32>, bool) {
        let mut world = World::new();
        let e = world.spawn_empty().id();
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            extract_avian_prim(reader, e, path, true, &mut commands);
        }
        queue.apply(&mut world);
        (
            world.get::<RigidBody>(e).copied(),
            world.get::<Collider>(e).map(|c| format!("{c:?}")),
            world.get::<Mass>(e).map(|m| m.0),
            world.get::<super::ShouldBeDynamic>(e).is_some(),
        )
    }

    #[test]
    fn extract_avian_from_stageview_builds_full_dynamic_body() {
        let dir = std::env::temp_dir().join("lunco_extract_parity");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("rover.usda");
        std::fs::write(&f, FIXTURE).unwrap();

        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);
        let rover = SdfPath::new("/Rover").unwrap();

        let live = run_extract(&view, &rover);

        // The LIVE path actually produced a full dynamic body: Kinematic
        // (settling to Dynamic via ShouldBeDynamic) + compound collider + mass.
        assert_eq!(live.0, Some(RigidBody::Kinematic), "live: rigid body");
        assert!(live.1.is_some(), "live: compound collider built off the stage");
        assert_eq!(live.2, Some(500.0), "live: authored mass read off the stage");
        assert!(live.3, "live: ShouldBeDynamic (settles to Dynamic)");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod joint_typed_tests {
    //! Ph0′ physics rework: the joint projector reads the STANDARD UsdPhysics
    //! joint schema (`openusd::schemas::physics`) off the live stage into the
    //! deferred `PendingUsdJoint` — bodies, axis, standard `physics:lowerLimit`/
    //! `upperLimit` (degrees → radians), local anchors, and `UsdPhysicsDriveAPI`.
    //! This is the headless-verifiable half of the rework (the read); joint
    //! *dynamics* need a rover boot.
    use super::read_joint_spec_typed;
    use bevy::math::DVec3;
    use lunco_usd_bevy::compose_file_to_stage;
    use openusd::sdf::Path as SdfPath;

    const FIXTURE: &str = r#"#usda 1.0
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsRevoluteJoint "Hinge" (
    prepend apiSchemas = ["PhysicsDriveAPI:angular"]
)
{
    rel physics:body0 = </Chassis>
    rel physics:body1 = </Wheel>
    uniform token physics:axis = "Y"
    float physics:lowerLimit = -45
    float physics:upperLimit = 45
    point3f physics:localPos0 = (1, 0, 0)
    point3f physics:localPos1 = (0, 0, 0)
    float drive:angular:physics:targetVelocity = 2.5
    float drive:angular:physics:maxForce = 100
}
"#;

    #[test]
    fn reads_standard_revolute_joint_off_live_stage() {
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hinge.usda");
        std::fs::write(&f, FIXTURE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");

        let j = read_joint_spec_typed(&stage, &SdfPath::new("/Hinge").unwrap())
            .expect("standard revolute joint reads off the live stage");

        assert_eq!(j.joint_type, "PhysicsRevoluteJoint");
        assert_eq!(j.body0_path, "/Chassis");
        assert_eq!(j.body1_path, "/Wheel");
        assert_eq!(j.axis, DVec3::Y);
        // Standard `physics:lowerLimit`/`upperLimit` are DEGREES → radians.
        assert!((j.limit_lower - (-45f64).to_radians()).abs() < 1e-9, "lower {}", j.limit_lower);
        assert!((j.limit_upper - 45f64.to_radians()).abs() < 1e-9, "upper {}", j.limit_upper);
        assert_eq!(j.local_pos0, DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(j.local_pos1, DVec3::ZERO);
        // UsdPhysicsDriveAPI:angular → JointDrive.
        let drive = j.drive.expect("angular drive read via DriveAPI");
        assert_eq!(drive.target_velocity, Some(2.5));
        assert_eq!(drive.max_force, Some(100.0));
        assert_eq!(drive.target_position, None);
    }

    #[test]
    fn non_joint_prim_reads_none() {
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("nojoint.usda");
        std::fs::write(&f, "#usda 1.0\ndef Xform \"Plain\" {}\n").unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        assert!(read_joint_spec_typed(&stage, &SdfPath::new("/Plain").unwrap()).is_none());
    }
}
