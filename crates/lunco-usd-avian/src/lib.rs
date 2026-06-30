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
    has_api_schema, read_rel_target, read_shape_dims, read_transform_from_usd,
    read_usd_mesh_indexed, usd_axis_to_quat, ShapeDims, UsdVisualSynced,
};
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::UsdData;

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
        app.add_observer(on_add_usd_prim)
            .add_observer(process_usd_avian_prims)
            .add_systems(
                Update,
                (
                    build_usd_physics_joints.run_if(any_with_component::<PendingUsdJoint>),
                    build_terrain_mesh_colliders
                        .run_if(any_with_component::<PendingTerrainCollider>),
                ),
            );
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
fn collect_child_colliders_from_usd(
    reader: &UsdData,
    parent_path: &SdfPath,
) -> Vec<(Position, Rotation, Collider)> {
    let mut shapes = Vec::new();

    for child_path in reader.prim_children(parent_path) {
        // Skip wheel children — they're independent dynamics handled
        // by `lunco-usd-sim` (raycast probe or physical wheel rigid
        // body), NOT collider pieces of the chassis compound. The
        // `physxVehicleWheel:radius` attribute is the canonical marker
        // (matches the same skip in `process_usd_avian_prims`).
        if reader.prim_attribute_value::<f32>(&child_path, "physxVehicleWheel:radius").is_some() {
            continue;
        }

        // Check if child has collision enabled
        let child_collision = reader
            .prim_attribute_value::<bool>(&child_path, "physics:collisionEnabled")
            .unwrap_or(true);
        if !child_collision { continue; }

        // Read child's local transform (canonical decoder, shared with usd-bevy).
        let mut child_tf = read_transform_from_usd(reader, &child_path);

        // For Cylinder children, fold UsdGeomCylinder.axis into the
        // child's compound-local rotation so the Y-axis collider lines
        // up with the authored axis (mirrors what lunco-usd-bevy does
        // for the entity Transform — same canonical `usd_axis_to_quat`).
        if let Some(ty) = reader.prim_type_name(&child_path) {
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
fn build_collider_from_usd(reader: &UsdData, sdf_path: &SdfPath) -> Option<Collider> {
    let ty = reader.prim_type_name(sdf_path)?;

    // Native UsdGeomMesh → static triangle-mesh collider, decoded from the
    // SAME `points`/`faceVertexIndices` `lunco-usd-bevy` renders (one geometry
    // source, so collider and visual can't drift). `set_scale` on a trimesh
    // scales its vertices exactly (no convex-hull tessellation), so the shared
    // scale tail applies unchanged.
    if ty == "Mesh" {
        let (verts, tris) = read_usd_mesh_indexed(reader, sdf_path)?;
        let verts: Vec<DVec3> =
            verts.into_iter().map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)).collect();
        return Some(apply_collider_scale(Collider::trimesh(verts, tris), reader, sdf_path));
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
fn apply_collider_scale(mut collider: Collider, reader: &UsdData, sdf_path: &SdfPath) -> Collider {
    let scale = read_vec3_attribute(reader, sdf_path, "xformOp:scale")
        .map(|v| (v.x, v.y, v.z))
        .unwrap_or((1.0, 1.0, 1.0));
    collider.set_scale(bevy::math::DVec3::new(scale.0, scale.1, scale.2), 10);
    collider
}

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd(
    commands: &mut Commands,
    entity: Entity,
    reader: &UsdData,
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
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

        // Borrow — `stage.reader` is `Arc<UsdData>`; deep-cloning it copies
        // the whole stage `HashMap`. Every read here is `&self`.
        let reader = &*stage.reader;

        // Skip wheel prims — the sim plugin handles those
        if reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius").is_some() {
            commands.entity(entity).insert(UsdAvianProcessed);
            return;
        }

        // Detect API schemas
        let has_rigid_body_api = has_api_schema(reader, &sdf_path, "PhysicsRigidBodyAPI");
        let has_collision_api = has_api_schema(reader, &sdf_path, "PhysicsCollisionAPI");
        let has_terrain_api = has_api_schema(reader, &sdf_path, "PhysxTerrainAPI");

        // ── TERRAIN HANDLING ──
        // Terrain is a static collider with the TerrainTile marker.
        if has_terrain_api {
            commands.entity(entity).insert((
                RigidBody::Static,
                lunco_terrain_globe::TerrainTile,
            ));
            // Primitive terrain (Cube/Sphere/Cylinder) → intrinsic collider.
            // Mesh terrain (a glTF DEM loaded via `lunco:assetMode = "mesh"`,
            // e.g. the Shackleton ridge) has no primitive shape, so its
            // collider is built from the loaded `Mesh3d` — deferred via
            // `PendingTerrainCollider` until the mesh asset finishes async-
            // loading. `build_terrain_mesh_colliders` then prefers a cheap
            // *heightfield* (the mesh is a regular DEM grid) and falls back
            // to a trimesh for irregular meshes. Either way rovers rest and
            // drive on the real surface instead of falling through.
            if let Some(collider) = build_collider_from_usd(reader, &sdf_path) {
                commands.entity(entity).insert(collider);
            } else {
                commands.entity(entity).insert(PendingTerrainCollider);
            }
            commands.entity(entity).insert(UsdAvianProcessed);
            return;
        }

        if has_rigid_body_api {
            // ── COMPOUND BODY ROOT ──
            // Read child collider shapes from USD and build compound collider
            let compound_shapes = collect_child_colliders_from_usd(reader, &sdf_path);

            if !compound_shapes.is_empty() {
                let compound = Collider::compound(compound_shapes);
                commands.entity(entity).insert(compound);
            } else {
                // No children with colliders — try this prim itself
                add_collider_from_usd(&mut commands, entity, reader, &sdf_path);
            }

            // Honour `bool physics:kinematicEnabled = true` for
            // bodies that should be externally controlled (gizmo,
            // scripts, MCP) without responding to gravity or impulses.
            // Kinematic bodies still participate in joint constraints
            // and contact events — that's the value here vs Static.
            let kinematic = reader
                .prim_attribute_value::<bool>(&sdf_path, "physics:kinematicEnabled")
                .unwrap_or(false);
            let body = if kinematic { RigidBody::Kinematic } else { RigidBody::Dynamic };
            commands.entity(entity).insert((
                body,
                lunco_core::SelectableRoot,
            ));

            // Map mass, damping, friction. Always insert a Mass —
            // `apply_gravity_to_rigid_bodies` filters on `With<Mass>`,
            // so a missing mass attribute (e.g. when the value lives on
            // a referenced base prim and openusd-rs's resolver doesn't
            // compose across the reference) would silently disable
            // gravity on the rover root. Default to 1000 kg, matching
            // the canonical rover mass authored in the base rover
            // .usda files.
            apply_rigid_body_mass_props(&mut commands, entity, reader, &sdf_path);

            commands.entity(entity).insert(UsdAvianProcessed);
        } else if has_collision_api {
            // ── COLLIDER CHILD ──
            // Part of parent's compound body — pure visual, no physics components.
            // Exception: root-level (no parent) → static collider.
            if q_child_of.get(entity).is_err() {
                commands.entity(entity).insert(RigidBody::Static);
                add_collider_from_usd(&mut commands, entity, reader, &sdf_path);
            }

            commands.entity(entity).insert(UsdAvianProcessed);
        } else {
            // ── FALLBACK: legacy physics:rigidBodyEnabled ──
            if let Some(true) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
                commands.entity(entity).insert((
                    RigidBody::Dynamic,
                    lunco_core::SelectableRoot,
                ));
                apply_rigid_body_mass_props(&mut commands, entity, reader, &sdf_path);
                add_collider_from_usd(&mut commands, entity, reader, &sdf_path);
            } else if let Some(false) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
                commands.entity(entity).insert(RigidBody::Static);
                add_collider_from_usd(&mut commands, entity, reader, &sdf_path);
            }

            commands.entity(entity).insert(UsdAvianProcessed);
        }
    }
}

/// Observer that fires when a USD prim entity is added.
///
/// Currently only detects physics joints (PhysicsPrismaticJoint, PhysicsRevoluteJoint,
/// etc.). Physics mapping for non-joint prims is handled by the deferred system.
fn on_add_usd_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    // Borrow, not deep-clone the `Arc<UsdData>` (whole-stage copy).
    let reader = &*stage.reader;

    // Skip wheel prims — the sim plugin handles those (raycast wheels don't need physical bodies)
    if reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius").is_some() {
        return;
    }

    // --- Detect Physics Joint Prims (PhysicsPrismaticJoint, PhysicsRevoluteJoint, etc.) ---
    if let Some(type_name) = reader.prim_type_name(&sdf_path) {
        if type_name.starts_with("Physics") && type_name.ends_with("Joint") {
                let body0 = read_rel_target(reader, &sdf_path, "physics:body0");
                let body1 = read_rel_target(reader, &sdf_path, "physics:body1");

                // Wheel-targeted joints are owned by `lunco-usd-sim` —
                // it spawns them synchronously inside `setup_physical_wheel`
                // alongside the wheel's `RigidBody`/`Collider`/`Motor`,
                // ensuring `JointCollisionDisabled` is in place before
                // any narrow-phase contact forms between wheel and
                // chassis. Building the same joint here would either
                // double up or race the wheel-body init.
                if let Some(b1) = body1.as_ref() {
                    if let Ok(b1_path) = SdfPath::new(b1) {
                        if reader.prim_attribute_value::<f32>(&b1_path, "physxVehicleWheel:radius").is_some() {
                            return;
                        }
                    }
                }

                match (body0, body1) {
                    (Some(body0_path), Some(body1_path)) => {
                        // OpenUSD standard: `UsdPhysicsRevoluteJoint.physics:axis`
                        // is a `uniform token` ("X" | "Y" | "Z"). Older
                        // authoring used a `physics:axis0` Vec3 — keep
                        // that as a fallback for any in-tree scenes
                        // that haven't been migrated yet.
                        let axis = read_token_attribute(reader, &sdf_path, "physics:axis")
                            .and_then(|t| match t.as_str() {
                                "X" => Some(DVec3::X),
                                "Y" => Some(DVec3::Y),
                                "Z" => Some(DVec3::Z),
                                _ => None,
                            })
                            .or_else(|| read_vec3_attribute(reader, &sdf_path, "physics:axis0"))
                            .unwrap_or(DVec3::Y);
                        // UsdPhysics `physics:localPos0/1` give the
                        // joint anchor on each body in that body's
                        // local frame. Without these, the joint forces
                        // both body centres to coincide — useful only
                        // when the bodies are co-located, which is
                        // rarely true in practice.
                        let local_pos0 = read_vec3_attribute(reader, &sdf_path, "physics:localPos0")
                            .unwrap_or(DVec3::ZERO);
                        let local_pos1 = read_vec3_attribute(reader, &sdf_path, "physics:localPos1")
                            .unwrap_or(DVec3::ZERO);
                        // Generic travel/angle limit. For a DistanceJoint the
                        // standard attrs are `physics:minDistance/maxDistance`;
                        // fall back to the generic limit names otherwise.
                        let limit_lower = read_scalar_attribute(reader, &sdf_path, "physics:minDistance")
                            .or_else(|| read_scalar_attribute(reader, &sdf_path, "physics:limitLower"))
                            .unwrap_or(f64::NEG_INFINITY);
                        let limit_upper = read_scalar_attribute(reader, &sdf_path, "physics:maxDistance")
                            .or_else(|| read_scalar_attribute(reader, &sdf_path, "physics:limitUpper"))
                            .unwrap_or(f64::INFINITY);
                        // Spherical swing cone (half-angles about the two axes
                        // orthogonal to the twist axis).
                        let swing_limit = read_scalar_attribute(reader, &sdf_path, "physics:coneAngle0Limit")
                            .zip(read_scalar_attribute(reader, &sdf_path, "physics:coneAngle1Limit"));

                        // `UsdPhysicsDriveAPI` instance is named for the DOF it
                        // drives: `linear` on a prismatic joint, `angular` on a
                        // revolute one. Absent → passive joint (wire-driven only).
                        let drive = match type_name.as_str() {
                            "PhysicsPrismaticJoint" => read_joint_drive(reader, &sdf_path, "linear"),
                            "PhysicsRevoluteJoint" => read_joint_drive(reader, &sdf_path, "angular"),
                            _ => None,
                        };

                        info!("Detected USD joint {} -> {} <-> {}", type_name, body0_path, body1_path);

                        commands.entity(entity).insert(PendingUsdJoint {
                            body0_path,
                            body1_path,
                            axis,
                            local_pos0,
                            local_pos1,
                            limit_lower,
                            limit_upper,
                            joint_type: type_name.clone(),
                            swing_limit,
                            drive,
                        });
                    }
                    (b0, b1) => {
                        warn!("Joint {} missing body refs: body0={:?} body1={:?}",
                            type_name, b0, b1);
                    }
                }
            }
    }

    // Note: Physics mapping (RigidBody, Mass, Collider, Damping) is handled by
    // the sim plugin's process_usd_sim_prims system to ensure consistent ordering
    // and avoid duplicate processing.
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
) {
    for (joint_entity, pending, joint_prim_path) in q_pending.iter() {
        // Find body0 and body1 entities by matching USD paths
        let body0_ent = q_bodies.iter()
            .find(|(_, path)| path.path == pending.body0_path
                && path.stage_handle == joint_prim_path.stage_handle)
            .map(|(e, _)| e);
        let body1_ent = q_bodies.iter()
            .find(|(_, path)| path.path == pending.body1_path
                && path.stage_handle == joint_prim_path.stage_handle)
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
                commands.entity(joint_entity).insert(joint);
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
                commands.entity(joint_entity).insert(joint);
            }
            "PhysicsFixedJoint" => {
                commands.entity(joint_entity).insert(
                    FixedJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1),
                );
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
                commands.entity(joint_entity).insert(joint);
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
                commands.entity(joint_entity).insert(
                    DistanceJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1)
                        .with_limits(min, max),
                );
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
fn read_token_attribute(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<String> {
    lunco_usd_bevy::read_token(reader, path, attr)
}

/// Reads a `DVec3` attribute (e.g., `double3 xformOp:translate`) at full
/// f64 precision.
///
/// Thin DVec3 adapter over the canonical [`lunco_usd_bevy::read_vec3_f64`]
/// (the 4-branch `[f32;3]→[f64;3]→Vec<f32>→Vec<f64>` ladder). Keeping the
/// reader f64 end-to-end is what avoids the documented silent-`None`
/// "bodies launched into orbit" bug for `physics:localPos*` anchors.
fn read_vec3_attribute(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<DVec3> {
    lunco_usd_bevy::read_vec3_f64(reader, path, attr).map(|v| DVec3::new(v[0], v[1], v[2]))
}

/// Reads a `UsdPhysicsDriveAPI` drive off a joint prim — the `drive:{ns}:physics:
/// {targetPosition,targetVelocity,maxForce}` attributes, where `ns` is `angular`
/// (revolute) or `linear` (prismatic). Returns `None` when none are authored, so
/// an undriven joint stays passive (wire-driven only).
fn read_joint_drive(reader: &UsdData, path: &SdfPath, ns: &str) -> Option<JointDrive> {
    let target_position = read_scalar_attribute(reader, path, &format!("drive:{ns}:physics:targetPosition"));
    let target_velocity = read_scalar_attribute(reader, path, &format!("drive:{ns}:physics:targetVelocity"));
    let max_force = read_scalar_attribute(reader, path, &format!("drive:{ns}:physics:maxForce"));
    if target_position.is_none() && target_velocity.is_none() && max_force.is_none() {
        return None;
    }
    Some(JointDrive {
        target_position,
        target_velocity,
        max_force,
    })
}

/// Reads a scalar attribute as `f64`, trying `float` first then `double`.
///
/// UsdPhysics/Omniverse authors most physics scalars as `float` (e.g.
/// `float drive:linear:physics:maxForce`), so a `::<f64>`-only read silently
/// returns `None` — the documented silent-`None` trap. Mirrors the f32-first
/// convention `lunco-usd-sim` uses for wheel/suspension attributes.
fn read_scalar_attribute(reader: &UsdData, path: &SdfPath, attr: &str) -> Option<f64> {
    reader
        .prim_attribute_value::<f32>(path, attr)
        .map(|v| v as f64)
        .or_else(|| reader.prim_attribute_value::<f64>(path, attr))
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
fn apply_rigid_body_mass_props(
    commands: &mut Commands,
    entity: Entity,
    reader: &UsdData,
    sdf_path: &SdfPath,
) {
    let mass = reader
        .prim_attribute_value::<f32>(sdf_path, "physics:mass")
        .or_else(|| {
            reader
                .prim_attribute_value::<f64>(sdf_path, "physics:mass")
                .map(|v| v as f32)
        })
        .unwrap_or(1000.0);
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

    if let Some(d) = reader.prim_attribute_value::<f32>(sdf_path, "physics:linearDamping") {
        commands.entity(entity).insert(LinearDamping(d as f64));
    }
    if let Some(d) = reader.prim_attribute_value::<f32>(sdf_path, "physics:angularDamping") {
        commands.entity(entity).insert(AngularDamping(d as f64));
    }
    if let Some(f) = reader.prim_attribute_value::<f32>(sdf_path, "physics:friction") {
        commands.entity(entity).insert(Friction::new(f.into()));
    }
}
