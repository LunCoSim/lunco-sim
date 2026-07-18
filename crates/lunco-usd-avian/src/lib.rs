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
//! | `material:binding:physics` → `PhysicsMaterialAPI` | `Friction`, `Restitution` | `physics:dynamicFriction` / `physics:staticFriction` / `physics:restitution` on the bound `Material`. There is no `physics:friction` attribute in UsdPhysics — see [`read_physics_material`]. |
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
    instance_key, local_transform_at, read_shape_dims, read_transform_from_usd,
    read_usd_mesh_indexed, usd_axis_to_quat, ShapeDims, StageView, UsdAnimated, UsdRead,
    UsdVisualSynced,
};
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset, UsdInstanceRoot};
use openusd::sdf::Path as SdfPath;
// UsdPhysics attribute + API-schema names as CONSTANTS, from openusd's own schema
// module. Hand-written `"physics:…"` string literals are how `physics:friction`
// (an attribute UsdPhysics does not define) got invented and lived here for
// months: a typo in a `&str` compiles.
use openusd::schemas::physics::tokens as ptok;
use openusd::usd::Stage;

pub mod big_space_bridge;
pub use big_space_bridge::{BigSpacePhysicsBridgePlugin, PhysicsBridgeSystems};

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
            // `build_usd_physics_joints` runs in avian's `PhysicsSystems::Prepare`,
            // NOT in `Update`, and that placement is load-bearing — it is the whole
            // reason the authored-joint path is safe.
            //
            // The window it has to hit: a joint may only be attached AFTER both
            // bodies are admitted to the island graph (else `merge_islands` panics
            // "Neither body … is in an island", which is what the `With<Position>`
            // gate below is for) but BEFORE the first narrow phase that could put
            // those bodies in contact (else the born-disabled `JointGraphEdge` comes
            // too late, `on_disable_joint_collision` deletes an already-touching
            // contact without unlinking it from its island, and a later island op
            // unwraps the freed `ContactId` — `islands/mod.rs:547`/`:608`).
            //
            // `Prepare` is exactly that window: it is chained before
            // `PhysicsSystems::StepSimulation`, which is what runs the broad/narrow
            // phase, while body admission (`RigidBody`'s required `Position` + the
            // `SolverBody`/`BodyIslandNode` hop) has already flushed. In `Update` the
            // gate could only open AFTER avian had already stepped — so the joint
            // always arrived a tick late, into live contacts. That is not a
            // hypothetical: `lunco-usd-sim`'s synthesized wheel joint hit this exact
            // race and was moved to a synchronous attach for it ("raced narrow-phase
            // contacts … crashing the Avian solver with 'Head contact has no
            // island'"); the authored path had the same bug and kept it.
            //
            // Within `Prepare` it must ALSO run after whatever makes `Position` REAL.
            // `Position` is a required component of `RigidBody`, so it EXISTS from the
            // moment the body spawns — holding its default of zero until something
            // derives it from the authored transform. The `With<Position>` gate
            // therefore proves admission to the island graph and nothing about the
            // pose being real. Read too early, every body is at (0,0,0), and the seat
            // below measures `localPos0 - localPos1` instead of the actual anchor
            // violation: a scene misplaced by metres is never corrected, a
            // correctly-placed one is nudged by the anchor offset. Both are silent —
            // the seat "succeeds" either way.
            //
            // WHICH system makes it real is the subtle part, and the reason the
            // obvious ordering did not work. In THIS app it is NOT avian's
            // `transform_to_position`: `BigSpacePhysicsBridgePlugin` sets
            // `PhysicsTransformConfig { transform_to_position: false, .. }`
            // (`big_space_bridge.rs`), and avian gates that system on exactly that
            // flag (`avian3d-0.7.0/src/physics_transform/mod.rs:108-110`). The system
            // never runs, `PhysicsTransformSystems::TransformToPosition` is an EMPTY
            // set, and ordering `.after` it is vacuous — measured: bodies still read
            // (0,0,0). The bridge owns the sync instead, in `pose_to_position`.
            //
            // The second half: `pose_to_position` lives in `PhysicsSchedule`, whereas
            // this system used to live in `FixedPostUpdate`. Those are different
            // schedules — `PhysicsSchedule` is run by avian's `run_physics_schedule`
            // from inside `FixedPostUpdate`'s `PhysicsSystems::StepSimulation`
            // (`avian3d-0.7.0/src/schedule/mod.rs:110-113`). A `FixedPostUpdate`
            // `Prepare` system is therefore ordered strictly BEFORE the whole physics
            // schedule, so no amount of `.after(...)` inside `FixedPostUpdate` could
            // ever have seen a bridge-written `Position`. Cross-schedule ordering is
            // silently a no-op, which is why the failure looked like a race.
            //
            // Moving into `PhysicsSchedule` fixes both and keeps the original window:
            // `.before(PhysicsStepSystems::First)` is still ahead of the broad/narrow
            // phase (stricter than the old placement), and `.after(pose_to_position)`
            // is now a REAL edge in a REAL shared schedule. When the bridge is absent
            // (plain-avian tests), `.after` degrades to a no-op — but then avian's own
            // `transform_to_position` is enabled and runs in `FixedPostUpdate`, i.e.
            // still before `PhysicsSchedule`. Correct in both configurations.
            .add_systems(
                avian3d::schedule::PhysicsSchedule,
                build_usd_physics_joints
                    .run_if(any_with_component::<PendingUsdJoint>)
                    .in_set(avian3d::prelude::PhysicsSystems::Prepare)
                    .after(avian3d::prelude::PhysicsSystems::First)
                    .after(big_space_bridge::PhysicsBridgeSystems::Read)
                    .before(avian3d::schedule::PhysicsStepSystems::First),
            )
            .add_systems(
                Update,
                (
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
        commands.entity(entity).try_insert(body);
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
                .try_insert((RigidBody::Kinematic, lunco_core::Mobility::Kinematic));
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
            .scalar::<bool>(&child_path, ptok::A_COLLISION_ENABLED)
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
                let axis_tok = reader
                    .text(&child_path, "axis")
                    .unwrap_or_else(|| "Z".to_string());
                // Pre-rotate by the stage convention: the `axis` token names an
                // axis of the STAGE's frame while the collider is built in the
                // canonical one (identical to what usd-bevy does for the visual
                // Transform, so mesh and collider can't disagree on a Z-up stage).
                let q_axis = lunco_usd_bevy::stage_convention(reader)
                    .orient(usd_axis_to_quat(&axis_tok).unwrap_or(Quat::IDENTITY));
                if !q_axis.abs_diff_eq(Quat::IDENTITY, 1e-6) {
                    child_tf.rotation = child_tf.rotation * q_axis;
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
        let collider = match reader.text(sdf_path, ptok::A_APPROXIMATION).as_deref() {
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
        ShapeDims::Cube { size } => Collider::cuboid(size, size, size),
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
        commands.entity(entity).try_insert(collider);
    }
}

/// True when some ancestor prim of `sdf_path` is a rigid body — i.e. this prim's
/// collider is a piece of that body's compound shape rather than a body (or
/// standalone static collider) in its own right.
///
/// Recognises both spellings of "this is a body": the standard `PhysicsRigidBodyAPI`
/// schema and the legacy `physics:rigidBodyEnabled` attribute that
/// [`extract_avian_prim`]'s fallback arm honours. Missing the legacy one would tear
/// the colliders off an old-style body and strand them as static geometry.
///
/// Walks the composed prim hierarchy, so it answers the same way off the live stage
/// or the flatten, and independently of where the prim happens to sit in the ECS.
fn has_rigid_body_ancestor<R: UsdRead>(reader: &R, sdf_path: &SdfPath) -> bool {
    let mut cur = sdf_path.parent();
    while let Some(p) = cur {
        if p.is_abs_root() {
            return false;
        }
        if reader.has_api_schema(&p, ptok::API_RIGID_BODY)
            || reader.scalar::<bool>(&p, ptok::A_RIGID_BODY_ENABLED) == Some(true)
        {
            return true;
        }
        cur = p.parent();
    }
    false
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
                commands.entity(entity).try_insert(c).remove::<PendingTerrainCollider>();
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
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

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
    extract_avian_prim(&cs.view(), entity, &sdf_path, &mut commands);
}

/// Map a single composed USD prim to its avian physics components, generic over
/// the read source ([`UsdRead`]) — so it drives off either the live canonical
/// [`StageView`](lunco_usd_bevy::StageView) or the flattened `sdf::Data`,
/// identically. Extracted from the observer for the Ph0′ cutover.
fn extract_avian_prim<R: UsdRead>(
    reader: &R,
    entity: Entity,
    sdf_path: &SdfPath,
    commands: &mut Commands,
) {
    // Skip wheel prims — the sim plugin handles those.
    if reader.real_f32(sdf_path, "physxVehicleWheel:radius").is_some() {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    let has_rigid_body_api = reader.has_api_schema(sdf_path, ptok::API_RIGID_BODY);
    let has_collision_api = reader.has_api_schema(sdf_path, ptok::API_COLLISION);
    let has_terrain_api = reader.has_api_schema(sdf_path, "LunCoTerrainAPI");

    // ── TERRAIN ── static collider + TerrainTile; mesh DEMs defer their collider.
    if has_terrain_api {
        commands.entity(entity).try_insert((
            RigidBody::Static,
            lunco_core::Mobility::Static,
            lunco_terrain_globe::TerrainTile,
        ));
        if let Some(collider) = build_collider_from_usd(reader, sdf_path) {
            commands.entity(entity).try_insert(collider);
        } else {
            commands.entity(entity).try_insert(PendingTerrainCollider);
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    // ── TRIGGER ZONE ── `lunco:triggerZone` → overlap-only static Sensor.
    if let Some(zone) = reader
        .scalar::<String>(sdf_path, "lunco:triggerZone")
        .filter(|z| !z.trim().is_empty())
    {
        commands.entity(entity).try_insert((RigidBody::Static, lunco_core::Mobility::Static));
        add_collider_from_usd(commands, entity, reader, sdf_path);
        commands.entity(entity).try_insert((
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
            commands.entity(entity).try_insert(Collider::compound(compound_shapes));
        } else {
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }

        // A `Dynamic`-declared body spawns `Kinematic` + `ShouldBeDynamic` and
        // settles to `Dynamic` once joints resolve (no 1-frame separation launch).
        let kinematic = reader.scalar::<bool>(sdf_path, ptok::A_KINEMATIC_ENABLED).unwrap_or(false);
        let (body, mobility) = if kinematic {
            (RigidBody::Kinematic, lunco_core::Mobility::Kinematic)
        } else {
            commands.entity(entity).try_insert(ShouldBeDynamic);
            (RigidBody::Kinematic, lunco_core::Mobility::Dynamic)
        };
        commands.entity(entity).try_insert((body, mobility, lunco_core::SelectableRoot));

        // Always insert a Mass (default 1000 kg) — gravity filters on `With<Mass>`.
        apply_rigid_body_mass_props(commands, entity, reader, sdf_path);
        commands.entity(entity).try_insert(UsdAvianProcessed);
    } else if has_collision_api {
        // ── COLLIDER PRIM, no body of its own ──
        // Per the USD physics spec, a collider belongs to the nearest ancestor
        // carrying `PhysicsRigidBodyAPI`, which folds it into that body's compound
        // shape (see the COMPOUND BODY ROOT arm above). Only when NO ancestor is a
        // rigid body does the collider stand alone — and then it is static geometry.
        //
        // Ancestry, not `is_root`, is the question: a ground plane authored one
        // level down (`/Scene/Ground` under a plain `Xform`) is every bit as
        // standalone as one at `/Ground`. Keying on root-ness silently gave such a
        // prim NO collider at all — things fell straight through the floor with no
        // error, and scenes worked around it by tacking on `LunCoTerrainAPI`.
        if !has_rigid_body_ancestor(reader, sdf_path) {
            commands.entity(entity).try_insert((RigidBody::Static, lunco_core::Mobility::Static));
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
    } else {
        // ── FALLBACK: legacy `physics:rigidBodyEnabled` ──
        if let Some(true) = reader.scalar::<bool>(sdf_path, ptok::A_RIGID_BODY_ENABLED) {
            commands.entity(entity).try_insert((
                RigidBody::Kinematic,
                lunco_core::Mobility::Dynamic,
                ShouldBeDynamic,
                lunco_core::SelectableRoot,
            ));
            apply_rigid_body_mass_props(commands, entity, reader, sdf_path);
            add_collider_from_usd(commands, entity, reader, sdf_path);
        } else if let Some(false) = reader.scalar::<bool>(sdf_path, ptok::A_RIGID_BODY_ENABLED) {
            commands.entity(entity).try_insert((RigidBody::Static, lunco_core::Mobility::Static));
            add_collider_from_usd(commands, entity, reader, sdf_path);
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
    }
}

/// The composed local-to-world [`Transform`] of `path`: folds the LOCAL transforms
/// (translate + rotate + **scale**) of every prim from the stage root down to it, so
/// an ancestor's scale is baked into a descendant's world position — exactly how the
/// renderer places it. Missing xform ops compose as identity.
fn world_transform<R: UsdRead>(reader: &R, path: &SdfPath) -> Transform {
    let mut chain = Vec::new();
    let mut cur = Some(path.clone());
    while let Some(p) = cur {
        if p.is_abs_root() {
            break;
        }
        chain.push(p.clone());
        cur = p.parent();
    }
    let mut acc = Transform::IDENTITY;
    for p in chain.iter().rev() {
        acc = acc.mul_transform(local_transform_at(reader, p, 0.0).unwrap_or(Transform::IDENTITY));
    }
    acc
}

/// Derive a joint's local anchors from the composed transform hierarchy, for the
/// **body1-origin** convention every rover joint uses: the joint sits at `body1`'s
/// origin, so its anchor on `body1` is the origin (`localPos1 = 0`) and its anchor on
/// `body0` is `body1`'s origin expressed in `body0`'s rotation frame. Returns
/// `(local_pos0, local_pos1)`.
///
/// This lets an asset author each part's placement ONCE — as the prim's
/// `xformOp:translate` — instead of typing it again as the joint's `physics:localPos0`.
/// `read_joint_spec_typed` calls it only when the anchor is UNAUTHORED, so authored
/// joints are untouched (no regression) and hand-tuned anchors always win.
///
/// Uses WORLD poses, not an ancestor walk, so it is correct for **sibling** joints
/// (a rocker ↔ bogie hinge where neither body contains the other) and for **scaled**
/// hierarchies: `localPos0 = rot(world(b0))⁻¹ · (pos(world(b1)) − pos(world(b0)))`.
/// Ancestor scales are baked into the world positions; the anchor is expressed in
/// body0's rotation frame (avian applies a body's rotation — not its scale — to a
/// local anchor). Relative, hence invariant under the reference/path-translation that
/// drops a shared component onto each rover root.
fn derive_joint_anchor<R: UsdRead>(reader: &R, body0: &str, body1: &str) -> Option<(DVec3, DVec3)> {
    let p0 = SdfPath::new(body0).ok()?;
    let p1 = SdfPath::new(body1).ok()?;
    let w0 = world_transform(reader, &p0);
    let w1 = world_transform(reader, &p1);
    let rel = w0.rotation.inverse() * (w1.translation - w0.translation);
    Some((DVec3::new(rel.x as f64, rel.y as f64, rel.z as f64), DVec3::ZERO))
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

    let view = StageView::new(stage);
    // **Units/axes convert here** (doc 41). `axis` names an axis of the STAGE's
    // frame, so on a Z-up stage an authored `"Z"` is *up* — canonical up is +Y.
    // Read raw it would hinge about the wrong axis while the meshes and colliders
    // (which do convert, via `local_transform_at`) sit correctly: a silently
    // wrong joint in a visually right assembly.
    let conv = lunco_usd_bevy::stage_convention(&view);
    let axis_of = |ax: Option<JointAxis>| {
        conv.dir_d(match ax.unwrap_or_default() {
            JointAxis::X => DVec3::X,
            JointAxis::Y => DVec3::Y,
            JointAxis::Z => DVec3::Z,
        })
    };
    // Shared JointBase reads (both bodies + local anchors). `None` unless BOTH
    // bodies are authored — world-anchored joints aren't mapped to avian here.
    // A missing anchor is DERIVED from the transform hierarchy (see
    // [`derive_joint_anchor`]) so an asset need not type the wheel's position twice
    // — once as its `xformOp:translate` and again as the joint's `localPos0`. An
    // authored anchor always wins.
    //
    // An AUTHORED anchor is a point in the stage's frame and units, so it converts
    // here. A DERIVED one must not: `derive_joint_anchor` builds it from
    // `world_transform` → `local_transform_at`, which already converted. Applying
    // the convention to both would double-convert the derived path.
    fn base<J: JointBase, R: UsdRead>(j: &J, reader: &R) -> Option<(String, String, DVec3, DVec3)> {
        let conv = lunco_usd_bevy::stage_convention(reader);
        let to_dvec =
            move |a: [f32; 3]| conv.point_d(DVec3::new(a[0] as f64, a[1] as f64, a[2] as f64));
        let b0 = j.body0_rel().targets().ok()?.into_iter().next()?.to_string();
        let b1 = j.body1_rel().targets().ok()?.into_iter().next()?.to_string();
        let lp0_auth = j.local_pos0_attr().get::<[f32; 3]>().ok().flatten().map(to_dvec);
        let lp1_auth = j.local_pos1_attr().get::<[f32; 3]>().ok().flatten().map(to_dvec);
        let (lp0, lp1) = if lp0_auth.is_none() || lp1_auth.is_none() {
            let derived = derive_joint_anchor(reader, &b0, &b1);
            (
                lp0_auth.or(derived.map(|d| d.0)).unwrap_or(DVec3::ZERO),
                lp1_auth.or(derived.map(|d| d.1)).unwrap_or(DVec3::ZERO),
            )
        } else {
            (lp0_auth.unwrap(), lp1_auth.unwrap())
        };
        Some((b0, b1, lp0, lp1))
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
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
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
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
        let axis = axis_of(j.axis_attr().get::<JointAxis>().ok().flatten());
        let lo = j.lower_limit_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::NEG_INFINITY);
        let hi = j.upper_limit_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::INFINITY);
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: "PhysicsPrismaticJoint".into(),
            swing_limit: None, drive: read_drive("linear"),
        }
    } else if let Some(j) = physics::SphericalJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
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
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis: DVec3::Y, local_pos0: lp0, local_pos1: lp1,
            limit_lower: f64::NEG_INFINITY, limit_upper: f64::INFINITY,
            joint_type: "PhysicsFixedJoint".into(), swing_limit: None, drive: None,
        }
    } else if let Some(j) = physics::DistanceJoint::get(stage, path.clone()).ok().flatten() {
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
        let lo = j.min_distance_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::NEG_INFINITY);
        let hi = j.max_distance_attr().get::<f32>().ok().flatten().map(|v| v as f64).unwrap_or(f64::INFINITY);
        PendingUsdJoint {
            body0_path: b0, body1_path: b1, axis: DVec3::Y, local_pos0: lp0, local_pos1: lp1,
            limit_lower: lo, limit_upper: hi, joint_type: "PhysicsDistanceJoint".into(),
            swing_limit: None, drive: None,
        }
    } else if let Some(j) = physics::Joint::get(stage, path.clone()).ok().flatten() {
        // Generic/D6 → reduce via per-DOF UsdPhysicsLimitAPI (typed).
        let (b0, b1, lp0, lp1) = base(&j, &view)?;
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
        commands.entity(entity).try_insert(joint);
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
/// Anchor mismatch below which a joint is considered already seated.
///
/// Sub-millimetre slack is float noise from the USD→physics transform chain, not a
/// scene error; correcting it would fight the solver on every reload.
const JOINT_SEAT_EPS: f64 = 1.0e-3;

/// Angular mismatch below which a weld is considered already seated (radians).
///
/// Same rationale as [`JOINT_SEAT_EPS`], in the rotational DOF: a milliradian is
/// quaternion round-tripping, not an authoring error.
const JOINT_SEAT_ANGLE_EPS: f64 = 1.0e-3;

/// Seat magnitude above which the scene is certainly wrong rather than slack.
///
/// A metre- or radian-scale correction is never authoring tolerance — it means
/// two bodies were placed inconsistently — and it must not be losable in a
/// normal log stream, so it is reported at `error!` instead of `warn!`.
const JOINT_SEAT_ERROR_THRESHOLD: f64 = 0.1;

fn build_usd_physics_joints(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingUsdJoint, &UsdPrimPath)>,
    // **Avian ADMISSION gate**: matching on `&Position` (added by
    // Avian's body-init systems alongside `BodyIslandNode`) ensures
    // we don't create a joint before Avian has admitted both bodies
    // into its island graph — without this the solver panics with
    // `Neither body … is in an island`. `process_usd_avian_prims`
    // queues the `RigidBody` insertion in our `Update`; Avian's
    // initialisation runs in its `PhysicsSchedule` (FixedUpdate),
    // so this query is empty for the first few frames after spawn,
    // and the joint stays in `PendingUsdJoint` until ready.
    //
    // Admission is NOT pose readiness. `Position` is a required component of
    // `RigidBody` and exists at its default zero from the instant the body
    // spawns, so this filter says nothing about the pose being real — that is
    // what `q_shadow` below is for. Conflating the two is the bug that made
    // joint seating measure `localPos0 - localPos1` for its whole life.
    q_bodies: Query<(Entity, &UsdPrimPath), With<Position>>,
    // **Pose readiness gate**: has the physics-transform bridge written a real
    // world pose into `Position` yet? See `BridgeShadow::is_seeded`.
    q_shadow: Query<&big_space_bridge::BridgeShadow>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    mut q_pose: Query<(&mut Position, &mut Rotation)>,
    mut q_vel: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
) {
    for (joint_entity, pending, joint_prim_path) in q_pending.iter() {
        let joint_root = instance_key(joint_entity, &q_provenance, &q_gid, &q_instance_root);
        // Find body0 and body1 entities by matching USD paths and instance roots
        let body0_ent = q_bodies.iter()
            .find(|(e, path)| {
                path.path == pending.body0_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && instance_key(*e, &q_provenance, &q_gid, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);
        let body1_ent = q_bodies.iter()
            .find(|(e, path)| {
                path.path == pending.body1_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && instance_key(*e, &q_provenance, &q_gid, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);

        let (Some(b0), Some(b1)) = (body0_ent, body1_ent) else { continue; };

        // Is `Position` the authored pose yet, or still `RigidBody`'s required-
        // component default of zero? Scheduling (see `UsdAvianPlugin`) puts this
        // system after the bridge's `pose_to_position`, so it normally is — but
        // "normally" is exactly what failed silently before, so the precondition is
        // CHECKED rather than assumed. A body the bridge has not reached stays
        // `PendingUsdJoint` for another tick instead of being welded against zeros;
        // this is the same deferral the admission gate above already relies on.
        //
        // `BridgeShadow::is_seeded` is the honest signal: the shadow starts as a NaN
        // sentinel and becomes finite exactly when the bridge first writes a real
        // world pose. An ABSENT shadow means `BigSpacePhysicsBridgePlugin` is not
        // installed, so avian's own `transform_to_position` owns `Position` and has
        // already run in `FixedPostUpdate` — ready by construction.
        //
        // This replaces a stopgap that inferred readiness from the two bodies being
        // coincident (`p0.distance_squared(p1) <= JOINT_SEAT_EPS` ⇒ "not real yet").
        // That heuristic was papering over the actual defect — cross-schedule
        // ordering against a system that never ran — and it is wrong in both
        // directions: it cannot see two bodies genuinely stacked at one origin, and
        // it calls uninitialised poses "ready" as soon as anything perturbs one.
        let seeded = |e: Entity| q_shadow.get(e).map(|s| s.is_seeded()).unwrap_or(true);
        if !seeded(b0) || !seeded(b1) {
            debug!(
                "[usd-avian] joint {} — body poses not seeded by the physics-transform \
                 bridge yet; deferring the joint rather than seating it against \
                 uninitialised positions.",
                joint_prim_path.path,
            );
            continue;
        }

        info!("Built USD joint {} -> {} <-> {}", pending.joint_type, pending.body0_path, pending.body1_path);

        // Seat the joint at its authored anchors before the solver sees it.
        //
        // The authored anchors ARE the joint: `physics:localPos0/1` say where the
        // two bodies are held together. A scene whose body transforms disagree with
        // them (overriding one body's `xformOp:translate` and not its partner's)
        // hands the solver a constraint violated by metres, which it resolves
        // impulsively — the bodies are yanked together and the pair explodes.
        //
        // Attachment is a KINEMATIC event, not a dynamic one, so `body1` moves to
        // satisfy the anchors and the solver starts from a consistent state. The
        // warning is deliberate: seating silently would hide a scene error whose
        // real fix belongs in the USD.
        //
        // Seating covers POSITION, ORIENTATION and VELOCITY, because a constraint
        // is violated in all three and the solver resolves each one impulsively.
        // Position alone is not enough: two bodies welded at a common anchor but
        // carrying different velocities must have that difference nulled in a
        // single step, and the resulting impulse acts at the anchor's lever arm
        // from each centre of mass — i.e. it arrives as a torque and the pair
        // tumbles. Seating position and then handing the solver a 7 m/s velocity
        // discontinuity trades an explosion for a slower explosion.
        //
        // Orientation and velocity are only seated for a WELD. `PhysicsFixedJoint`
        // is built with identity `JointFrame`s on both bodies, so it holds
        // `rot1 == rot0` and the two move as one rigid body — both corrections are
        // then unambiguous. Every other joint type leaves rotational or linear DOF
        // free by design, and forcing agreement across a free DOF would destroy
        // authored state (a revolute joint's whole purpose is that the bodies'
        // orientations differ). For those, position remains the only safe seat.
        let rigid = pending.joint_type == "PhysicsFixedJoint";

        let pose0 = q_pose.get(b0).ok().map(|(p, r)| (p.0, r.0));
        let pose1 = q_pose.get(b1).ok().map(|(p, r)| (p.0, r.0));

        if let (Some((p0, r0)), Some((p1, r1))) = (pose0, pose1) {
            // Seat orientation FIRST, then measure position against the corrected
            // orientation: rotating body1 swings its anchor through `local_pos1`,
            // so a delta computed from the old rotation would leave a residual
            // exactly as large as that swing.
            let r1_seated = if rigid { r0 } else { r1 };
            let angle = if rigid { r1.angle_between(r0) } else { 0.0 };

            let anchor0_world = p0 + r0 * pending.local_pos0;
            let anchor1_world = p1 + r1_seated * pending.local_pos1;
            let delta = anchor0_world - anchor1_world;

            // Sub-millimetre / sub-milliradian slack is just float noise from the
            // USD→physics transform chain; correcting it would fight the solver
            // every reload.
            let seat_pos = delta.length() > JOINT_SEAT_EPS;
            let seat_rot = angle > JOINT_SEAT_ANGLE_EPS;

            if seat_pos || seat_rot {
                let worst = delta.length().max(angle);
                // The anchors are printed because a violation is ambiguous without
                // them: the same delta arises from bodies placed wrongly AND from
                // anchors that failed to read and defaulted to zero, and those have
                // opposite fixes. Zeros here with a non-zero delta mean the anchor
                // read/derive fell through, not that the scene is misplaced.
                let detail = format!(
                    "[usd-avian] joint {} starts violated by {:.3} m / {:.3} rad — seating \
                     `{}` onto the authored anchor. anchors: localPos0={:?} localPos1={:?}, \
                     body0 at {:?}, body1 at {:?}. (Check `xformOp:translate`, any \
                     rotate/orient op, and `physics:velocity` on BOTH bodies against \
                     `physics:localPos0/1`.)",
                    joint_prim_path.path,
                    delta.length(),
                    angle,
                    pending.body1_path,
                    pending.local_pos0,
                    pending.local_pos1,
                    p0,
                    p1,
                );
                // A metre-scale seat is a scene bug every time; do not let it hide
                // among ordinary warnings.
                if worst > JOINT_SEAT_ERROR_THRESHOLD {
                    error!("{detail}");
                } else {
                    warn!("{detail}");
                }

                if let Ok((mut pos1, mut rot1)) = q_pose.get_mut(b1) {
                    if seat_rot {
                        rot1.0 = r0;
                    }
                    if seat_pos {
                        pos1.0 += delta;
                    }
                }

                // Match body1's motion to body0's rigid motion about the seated
                // pose. `v = v0 + ω0 × r` is the velocity of the point of body0
                // that body1's centre now coincides with — the only assignment
                // consistent with a weld.
                if rigid {
                    let motion0 = q_vel.get(b0).ok().map(|(l, a)| (l.0, a.0));
                    if let Some((lin0, ang0)) = motion0 {
                        let p1_seated = p1 + delta;
                        let target_lin = lin0 + ang0.cross(p1_seated - p0);
                        if let Ok((mut lin1, mut ang1)) = q_vel.get_mut(b1) {
                            if (lin1.0 - target_lin).length() > JOINT_SEAT_EPS
                                || (ang1.0 - ang0).length() > JOINT_SEAT_ANGLE_EPS
                            {
                                lin1.0 = target_lin;
                                ang1.0 = ang0;
                            }
                        }
                    }
                }
            }
        }

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
                commands.entity(joint_entity).try_insert(joint_bundle(joint));
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
                commands.entity(joint_entity).try_insert(joint_bundle(joint));
            }
            "PhysicsFixedJoint" => {
                commands.entity(joint_entity).try_insert(joint_bundle(
                    FixedJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1),
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
                commands.entity(joint_entity).try_insert(joint_bundle(joint));
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
                commands.entity(joint_entity).try_insert(joint_bundle(
                    DistanceJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1)
                        .with_limits(min, max),
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
/// THE ONLY sanctioned way to hand an Avian joint to the world. Every joint in
/// this workspace — authored USD joints here, the synthesized wheel joint in
/// `lunco-usd-sim` — goes through this.
///
/// It exists to make ONE avian rule un-forgettable: **`JointCollisionDisabled`
/// must ride the same bundle as the joint component, never a later insert.**
///
/// Why the bundle, specifically. Bevy writes a whole bundle before firing any
/// hook or observer, so `add_joint_to_graph` (`joint_graph/plugin.rs:135-143`)
/// reads `Has<JointCollisionDisabled> == true` and the `JointGraphEdge` is BORN
/// with `collision_disabled`. The broad phase then never creates the pair at all
/// (`bvh_broad_phase.rs:275-283`), so no contact between the jointed bodies ever
/// exists. Add the marker one command later and you take the other road:
/// `on_disable_joint_collision` (`joint_graph/plugin.rs:290-295`) walks the
/// EXISTING contacts and deletes them with `remove_edge_by_id` while never
/// calling `IslandManager::remove_contact` — leaving a freed `ContactId` in the
/// island's linked list, which a later island op unwraps and dies on
/// (`islands/mod.rs:547`/`:608`). That is an upstream avian bug we cannot patch
/// from here; the bundle is how we stay out of its reach.
///
/// This is only half the contract. The other half is TIMING and it belongs to
/// the caller: the bundle must land BEFORE the first narrow phase that could put
/// the two bodies in contact. Born-disabled prevents the pair from ever forming;
/// it does NOT clean up a contact that already exists — and if one does, this
/// bundle walks straight into the same corrupting path. See
/// `crates/lunco-usd-avian/tests/gizmo_body_swap_islands.rs`, where
/// `joint_and_collision_disabled_inserted_as_one_bundle` panics for exactly that
/// reason: correct bundle, too late.
pub fn joint_bundle<J: Component>(joint: J) -> (J, JointCollisionDisabled) {
    (joint, JointCollisionDisabled)
}

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
    let mass = reader.real_f32(sdf_path, ptok::A_MASS).unwrap_or(1000.0);
    commands.entity(entity).try_insert(Mass(mass));

    // G2 — authored principal inertia. `physics:diagonalInertia` is the diagonal
    // of the inertia tensor in the principal frame. `physics:principalAxes` (a
    // quat) would rotate that frame; it's almost always identity for
    // landers/rovers and is left to default. Off-diagonal inertia is not
    // representable here (Avian stores principal + frame), matching the
    // UsdPhysics schema.
    if let Some(diag) = read_vec3_attribute(reader, sdf_path, ptok::A_DIAGONAL_INERTIA) {
        commands.entity(entity).try_insert(AngularInertia {
            principal: diag.as_vec3(),
            local_frame: Quat::IDENTITY,
        });
    }

    // G2 — authored centre of mass (body-frame offset).
    if let Some(com) = read_vec3_attribute(reader, sdf_path, ptok::A_CENTER_OF_MASS) {
        commands.entity(entity).try_insert(CenterOfMass(com.as_vec3()));
    }

    if let Some(d) = reader.real_f32(sdf_path, PHYSX_LINEAR_DAMPING) {
        commands.entity(entity).try_insert(LinearDamping(d as f64));
    }
    if let Some(d) = reader.real_f32(sdf_path, PHYSX_ANGULAR_DAMPING) {
        commands.entity(entity).try_insert(AngularDamping(d as f64));
    }
    // Friction/restitution come from a bound `UsdPhysicsMaterialAPI` material —
    // NOT from a `physics:friction` attribute on the body, which is not a thing
    // UsdPhysics defines (see `read_physics_material`).
    //
    // USD and Avian BOTH model dynamic and static friction separately, so map
    // them across one-to-one rather than collapsing to a single coefficient.
    // Either may be unauthored; fall back to Avian's own default for that one
    // (0.5), not to the other coefficient — "sticky but slippery" is a legitimate
    // surface, and silently mirroring one onto the other would erase it.
    //
    // How the ROVER's friction and the GROUND's friction interact: they don't
    // average in USD — USD only says what each surface IS. The pairing happens in
    // the solver, per contact: Avian takes the two bodies' `Friction` components
    // and combines each coefficient with the `combine_rule` (default `Average`,
    // matching PhysX). So regolith at 1.0 under a wheel at 0.8 yields 0.9 unless
    // a material says otherwise — and a material CAN say otherwise, via
    // `physxMaterial:frictionCombineMode` (`min` is the usual choice when you
    // want "the slipperiest surface wins", which is the physically honest rule
    // for a wheel on dust).
    if let Some(pm) = read_physics_material(reader, sdf_path) {
        if pm.dynamic_friction.is_some() || pm.static_friction.is_some() {
            let d = Friction::default();
            commands.entity(entity).try_insert(Friction {
                dynamic_coefficient: pm
                    .dynamic_friction
                    .map_or(d.dynamic_coefficient, |f| f.into()),
                static_coefficient: pm
                    .static_friction
                    .map_or(d.static_coefficient, |f| f.into()),
                combine_rule: pm.friction_combine.unwrap_or(d.combine_rule),
            });
        }
        if let Some(r) = pm.restitution {
            let d = Restitution::default();
            commands.entity(entity).try_insert(Restitution {
                coefficient: r.into(),
                combine_rule: pm.restitution_combine.unwrap_or(d.combine_rule),
            });
        }
    }
    if let Some(vel) = read_vec3_attribute(reader, sdf_path, ptok::A_VELOCITY) {
        commands.entity(entity).try_insert(LinearVelocity(vel));
    }
    if let Some(ang) = read_vec3_attribute(reader, sdf_path, ptok::A_ANGULAR_VELOCITY) {
        commands.entity(entity).try_insert(AngularVelocity(ang));
    }
}

/// Damping is **not** a UsdPhysics concept — the core spec has no damping
/// attribute at all. Omniverse contributes it via `PhysxRigidBodyAPI`, and these
/// are its names. We used to author `physics:linearDamping`, squatting the
/// UsdPhysics namespace with an attribute it does not define.
const PHYSX_LINEAR_DAMPING: &str = "physxRigidBody:linearDamping";
const PHYSX_ANGULAR_DAMPING: &str = "physxRigidBody:angularDamping";

/// How two contacting surfaces' coefficients are combined — `PhysxMaterialAPI`'s
/// `physxMaterial:frictionCombineMode`. Also not core UsdPhysics: the spec says
/// what a surface IS, and leaves the pairwise combination to the solver. This is
/// Omniverse's (and PhysX's) name for it, and Avian implements the same rules.
const PHYSX_FRICTION_COMBINE_MODE: &str = "physxMaterial:frictionCombineMode";
const PHYSX_RESTITUTION_COMBINE_MODE: &str = "physxMaterial:restitutionCombineMode";

/// PhysX/Omniverse combine-mode token → Avian's [`CoefficientCombine`].
///
/// `average` is the default in both, so an unauthored mode behaves identically.
/// (Avian additionally offers `GeometricMean`, which PhysX has no token for; it
/// is reachable only from Rust.)
fn combine_mode(token: Option<&str>) -> Option<CoefficientCombine> {
    match token? {
        "average" => Some(CoefficientCombine::Average),
        "min" => Some(CoefficientCombine::Min),
        "multiply" => Some(CoefficientCombine::Multiply),
        "max" => Some(CoefficientCombine::Max),
        other => {
            bevy::log::warn!(
                "[usd-avian] unknown frictionCombineMode `{other}` — expected \
                 average/min/multiply/max; using the default (average)"
            );
            None
        }
    }
}

/// The surface properties of a bound `UsdPhysicsMaterialAPI` material.
///
/// Dynamic and static friction are kept **separate**, because both USD and Avian
/// model them separately (`physics:dynamicFriction` / `physics:staticFriction`;
/// `Friction::dynamic_coefficient` / `static_coefficient`). Collapsing them to
/// one number — as the old `physics:friction` did — throws away the distinction
/// between "how hard is it to start sliding" and "how hard is it to keep
/// sliding", which for a rover on regolith is exactly the interesting part.
pub struct PhysicsMaterial {
    /// `physics:dynamicFriction` — kinetic, while surfaces slide.
    pub dynamic_friction: Option<f32>,
    /// `physics:staticFriction` — resists the onset of sliding.
    pub static_friction: Option<f32>,
    /// `physics:restitution` — bounciness.
    pub restitution: Option<f32>,
    /// `physxMaterial:frictionCombineMode` — how THIS surface's friction combines
    /// with whatever it touches.
    pub friction_combine: Option<CoefficientCombine>,
    /// `physxMaterial:restitutionCombineMode`.
    pub restitution_combine: Option<CoefficientCombine>,
}

/// Resolve the physics material bound to `prim` and read its surface properties.
///
/// # Why this is not just an attribute read
///
/// There is no `physics:friction` in UsdPhysics. Friction is
/// `UsdPhysicsMaterialAPI` — `physics:dynamicFriction` / `physics:staticFriction`
/// / `physics:restitution` / `physics:density` — applied to a **`Material`** prim
/// and bound to geometry through the purpose-specific relationship
/// `material:binding:physics`:
///
/// ```usda
/// def Scope "PhysicsMaterials" {
///     def Material "Regolith" (prepend apiSchemas = ["PhysicsMaterialAPI"]) {
///         float physics:dynamicFriction = 1.0
///         float physics:staticFriction  = 1.0
///     }
/// }
/// def Cube "Ground" (prepend apiSchemas = ["PhysicsCollisionAPI"]) {
///     rel material:binding:physics = </World/PhysicsMaterials/Regolith>
/// }
/// ```
///
/// We used to read a bare `physics:friction` off the body prim: an invented
/// attribute inside a namespace UsdPhysics owns. Omniverse and every other
/// physics-aware consumer ignored it, and had USD ever defined that name, our
/// value would have been silently reinterpreted.
///
/// Binding resolution — namespace inheritance, and the purpose→all-purpose
/// fallback that lets ONE `Material` drive both look and friction — is SHARED
/// with the renderer ([`lunco_usd_bevy::resolve_bound_material`]). A physical and
/// a visual material are the same USD concept bound for different purposes, so
/// they must resolve through the same code or they will drift.
pub fn read_physics_material<R: UsdRead>(reader: &R, prim: &SdfPath) -> Option<PhysicsMaterial> {
    use openusd::schemas::physics::tokens as ptok;

    let mat = lunco_usd_bevy::resolve_bound_material(
        reader,
        prim,
        lunco_usd_bevy::MaterialPurpose::Physics,
    )?;
    let dynamic_friction = reader.real_f32(&mat, ptok::A_DYNAMIC_FRICTION);
    let static_friction = reader.real_f32(&mat, ptok::A_STATIC_FRICTION);
    let restitution = reader.real_f32(&mat, ptok::A_RESTITUTION);
    let friction_combine =
        combine_mode(reader.text(&mat, PHYSX_FRICTION_COMBINE_MODE).as_deref());
    let restitution_combine =
        combine_mode(reader.text(&mat, PHYSX_RESTITUTION_COMBINE_MODE).as_deref());

    // A Material bound only for LOOKS resolves here via the purpose→all-purpose
    // fallback but carries no `PhysicsMaterialAPI` properties. That is not a
    // physics material — don't fabricate a zero-friction one out of it.
    (dynamic_friction.is_some() || static_friction.is_some() || restitution.is_some()).then_some(
        PhysicsMaterial {
            dynamic_friction,
            static_friction,
            restitution,
            friction_combine,
            restitution_combine,
        },
    )
}

/// Marker component to hold a rigid body as Kinematic until all joints
/// and constraints are fully resolved in the stage, preventing 1-frame
/// physics separation explosions.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShouldBeDynamic;

// USDA fixtures are written to a temp dir and composed from disk. Native-only
// test code: the `disallowed_methods` ban on `std::fs` guards wasm *runtime*
// paths (clippy.toml names `tests/` as exempt; cargo has no path-scoped lint
// config, so the exemption is written out).
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)]
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
#[allow(clippy::disallowed_methods)] // temp-dir USDA fixtures; see `collider_parity_tests`
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
            extract_avian_prim(reader, e, path, &mut commands);
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
#[allow(clippy::disallowed_methods)] // temp-dir USDA fixtures; see `collider_parity_tests`
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

    /// A Z-up / centimetre stage — the Omniverse and Isaac Sim default — must
    /// convert the joint's AXIS and its AUTHORED anchors, exactly as meshes and
    /// colliders already do through `local_transform_at`.
    ///
    /// Before doc 41's conversion reached this reader, both were taken raw: the
    /// hinge rotated about the stage's +Z while the canonical frame's up is +Y,
    /// and a 100 cm anchor stayed "100 m". Meshes and colliders converted
    /// correctly, so the assembly LOOKED right and only the physics was wrong —
    /// the failure mode a regression test has to pin down.
    const ZUP_CM_FIXTURE: &str = r#"#usda 1.0
(
    upAxis = "Z"
    metersPerUnit = 0.01
)
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsRevoluteJoint "Hinge"
{
    rel physics:body0 = </Chassis>
    rel physics:body1 = </Wheel>
    uniform token physics:axis = "Z"
    point3f physics:localPos0 = (0, 0, 100)
    point3f physics:localPos1 = (0, 0, 0)
}
"#;

    #[test]
    fn zup_centimetre_stage_converts_joint_axis_and_authored_anchors() {
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hinge_zup_cm.usda");
        std::fs::write(&f, ZUP_CM_FIXTURE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");

        let j = read_joint_spec_typed(&stage, &SdfPath::new("/Hinge").unwrap())
            .expect("revolute joint reads off a Z-up stage");

        // Tolerance is 1e-6, not machine epsilon: `ConventionTransform` stores its
        // up-axis rotation as an `f32` `Quat`, so `Rx(-90°)` carries ~3e-8 of f32
        // error that `point_d`/`dir_d` faithfully propagate. That is the real
        // guarantee — the f64 arms preserve the precision of the INPUT and of the
        // metres-per-unit multiply, not the rotation's own accuracy. 1e-6 still
        // catches the bug this test exists for: an unconverted axis is off by a
        // full 90°, not 3e-8.
        //
        // `axis = "Z"` names the STAGE's up. Canonical up is +Y, so Rx(-90°)
        // must carry it there: (x,y,z) -> (x, z, -y).
        assert!(
            (j.axis - DVec3::Y).length() < 1e-6,
            "joint axis not converted to canonical: {:?} (want +Y)",
            j.axis
        );

        // Anchor (0,0,100) cm -> Q*(0,0,100) = (0,100,0), x0.01 -> (0,1,0) m.
        let want = DVec3::new(0.0, 1.0, 0.0);
        assert!(
            (j.local_pos0 - want).length() < 1e-6,
            "authored localPos0 not converted: {:?} (want {want:?})",
            j.local_pos0
        );
        assert_eq!(j.local_pos1, DVec3::ZERO, "origin anchor stays the origin");
    }

    /// Anchors round-trip through `[f32;3]` both when authored and when derived, so
    /// compare at f32 precision — the point is that the derived value equals what the
    /// file used to hand-author (byte-identical physics), not full f64 equality.
    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-5
    }

    fn write_and_compose(name: &str, body: &str) -> openusd::usd::Stage {
        let dir = std::env::temp_dir().join("lunco_joint_derive");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(name);
        std::fs::write(&f, body).unwrap();
        compose_file_to_stage(&f).expect("compose stage")
    }

    const DERIVE_FIXTURE: &str = "#usda 1.0\n\
def Xform \"Rover\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n{\n\
    def Xform \"Wheel\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n    {\n\
        double3 xformOp:translate = (0.9, -0.65, 1.225)\n\
        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n\
    def PhysicsRevoluteJoint \"Hinge\"\n    {\n\
        rel physics:body0 = </Rover>\n\
        rel physics:body1 = </Rover/Wheel>\n\
        uniform token physics:axis = \"X\"\nAUTHORED    }\n}\n";

    #[test]
    fn derives_unauthored_joint_anchor_from_child_translate() {
        // A wheel placed by its own `xformOp:translate`, jointed to the root with NO
        // `physics:localPos0/1`. The reader must DERIVE the anchor: lp0 = the wheel's
        // origin in the root frame (its translate), lp1 = origin. This is what lets
        // `physical_drivetrain.usda` state each wheel's position once, not twice.
        let stage = write_and_compose("derive.usda", &DERIVE_FIXTURE.replace("AUTHORED", ""));
        let j = read_joint_spec_typed(&stage, &SdfPath::new("/Rover/Hinge").unwrap())
            .expect("revolute joint reads");
        assert!(close(j.local_pos0, DVec3::new(0.9, -0.65, 1.225)), "lp0 derived from wheel translate: {:?}", j.local_pos0);
        assert_eq!(j.local_pos1, DVec3::ZERO, "lp1 = body1 origin");
    }

    #[test]
    fn authored_anchor_is_not_overridden_by_derivation() {
        // An explicit `physics:localPos0` must win — the derivation is a fallback for
        // UNAUTHORED anchors only, so hand-tuned joints never change.
        let stage = write_and_compose(
            "authored.usda",
            &DERIVE_FIXTURE.replace("AUTHORED", "        point3f physics:localPos0 = (1, 2, 3)\n"),
        );
        let j = read_joint_spec_typed(&stage, &SdfPath::new("/Rover/Hinge").unwrap())
            .expect("revolute joint reads");
        assert_eq!(j.local_pos0, DVec3::new(1.0, 2.0, 3.0), "authored lp0 wins over derivation");
    }

    #[test]
    fn rocker_bogie_hinge_joints_derive_end_to_end() {
        // The HARD retrofit, through the real load path. `rocker_bogie.usda` now omits
        // every anchor. Its FOUR structural hinges flow through `read_joint_spec_typed`
        // and must be DERIVED — including the two SIBLING bogie hinges (`BogieHinge*`:
        // body0 does NOT contain body1) and a scaled hierarchy — reproducing the values
        // the file used to hand-author (byte-identical → unchanged physics).
        //
        // (The six WHEEL joints are `physxVehicleWheel`-tagged and owned by
        // `lunco-usd-sim`, which builds them from the wheel's own transform —
        // `mount_local = existing_tf.translation`, never reading `localPos0`. So those
        // dropped anchors were already dead there; nothing to derive here.)
        let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/vessels/rovers/rocker_bogie.usda");
        let stage = compose_file_to_stage(&f).expect("compose rocker_bogie");
        for (name, lp0) in [
            ("HingeL", [-0.9, -0.2, 0.0]),       // chassis ↔ rocker (ancestor)
            ("HingeR", [0.9, -0.2, 0.0]),
            ("BogieHingeL", [0.0, -0.2, 0.6]),   // rocker ↔ bogie (SIBLING)
            ("BogieHingeR", [0.0, -0.2, 0.6]),
        ] {
            let j = read_joint_spec_typed(&stage, &SdfPath::new(&format!("/RockerBogie/{name}")).unwrap())
                .unwrap_or_else(|| panic!("{name} reads + derives"));
            assert!(
                close(j.local_pos0, DVec3::new(lp0[0], lp0[1], lp0[2])),
                "{name}: derived {:?} != old authored {lp0:?}",
                j.local_pos0
            );
            assert_eq!(j.local_pos1, DVec3::ZERO, "{name}: lp1 = origin");
        }
    }

    #[test]
    fn physical_drivetrain_derives_all_four_wheel_anchors() {
        // The shipped retrofit: `physical_drivetrain.usda` now OMITS every
        // localPos0/1. The reader must reproduce, exactly, the four wheel anchors the
        // file used to type twice — proving the retrofit preserves the physics.
        let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/components/mobility/physical_drivetrain.usda");
        let stage = compose_file_to_stage(&f).expect("compose drivetrain");
        for (name, lp0) in [
            ("Wheel_FL_Hinge", DVec3::new(-0.9, -0.65, -1.225)),
            ("Wheel_FR_Hinge", DVec3::new(0.9, -0.65, -1.225)),
            ("Wheel_RL_Hinge", DVec3::new(-0.9, -0.65, 1.225)),
            ("Wheel_RR_Hinge", DVec3::new(0.9, -0.65, 1.225)),
        ] {
            let j = read_joint_spec_typed(&stage, &SdfPath::new(&format!("/Drivetrain/{name}")).unwrap())
                .unwrap_or_else(|| panic!("{name} reads"));
            assert!(close(j.local_pos0, lp0), "{name}: anchor derived from the wheel over-translate: {:?}", j.local_pos0);
            assert_eq!(j.local_pos1, DVec3::ZERO, "{name}: lp1 = origin");
        }
    }
}

#[cfg(test)]
mod collider_ownership_tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    /// A ground plane one level under a plain `Xform` (the shape every scene and
    /// tutorial authors), plus a rigid-body lander whose only collider is its own
    /// root geometry, plus a lander with a collider CHILD.
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "Mission"
)
def Xform "Mission"
{
    def Cube "Ground" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        double size = 1.0
        bool physics:collisionEnabled = true
    }

    def Cylinder "BareLander" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] )
    {
        uniform token axis = "Y"
        double radius = 2.5
        double height = 3.0
        bool physics:rigidBodyEnabled = true
        bool physics:collisionEnabled = true
    }

    def Xform "LegacyBody"
    {
        bool physics:rigidBodyEnabled = true

        def Cube "Shell" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
        {
            double size = 1.0
            bool physics:collisionEnabled = true
        }
    }

    def Cylinder "CompoundLander" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] )
    {
        uniform token axis = "Y"
        double radius = 2.5
        double height = 3.0
        bool physics:rigidBodyEnabled = true

        def Cylinder "Hull" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
        {
            uniform token axis = "Y"
            double radius = 2.5
            double height = 3.0
            bool physics:collisionEnabled = true
        }
    }
}
"#;

    /// Run the extractor on one prim and return its resulting components.
    fn extract(view: &lunco_usd_bevy::StageView, path: &str) -> (bool, Option<RigidBody>) {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let sdf = SdfPath::new(path).unwrap();
        {
            let mut commands = world.commands();
            extract_avian_prim(view, entity, &sdf, &mut commands);
        }
        world.flush();
        (world.get::<Collider>(entity).is_some(), world.get::<RigidBody>(entity).copied())
    }

    /// The regression this exists for: a collider prim with no rigid-body ancestor
    /// is standalone STATIC geometry — even when it is not an ECS root. Keying this
    /// off root-ness gave `/Mission/Ground` no collider at all, silently, and
    /// everything that landed on it fell through the world.
    #[test]
    fn nested_collider_without_rigid_body_ancestor_is_static_geometry() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let (has_collider, body) = extract(&cs.view(), "/Mission/Ground");
        assert!(has_collider, "a ground plane under an Xform must get a collider");
        assert_eq!(body, Some(RigidBody::Static), "and it must be static");
    }

    /// The other half of the rule: a collider UNDER a rigid body is a piece of that
    /// body's compound shape, so it gets no collider and no body of its own.
    #[test]
    fn collider_under_rigid_body_ancestor_stays_a_compound_piece() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let (has_collider, body) = extract(&cs.view(), "/Mission/CompoundLander/Hull");
        assert!(!has_collider, "a collider child must not carry its own collider");
        assert_eq!(body, None, "nor its own rigid body");
    }

    /// A rigid-body root with NO collider children falls back to its own geometry.
    /// (It always did; asserted here so the compound arm can never quietly eat it.)
    #[test]
    fn rigid_body_root_without_collider_children_uses_its_own_shape() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let lander = SdfPath::new("/Mission/BareLander").unwrap();
        let view = cs.view();
        assert!(collect_child_colliders_from_usd(&view, &lander).is_empty());
        assert!(build_collider_from_usd(&view, &lander).is_some());
        let (has_collider, _) = extract(&view, "/Mission/BareLander");
        assert!(has_collider, "a bare rigid-body root must collide via its own shape");
    }

    /// A body declared the legacy way (`physics:rigidBodyEnabled`, no API schema)
    /// still owns its collider children — they must not become static geometry.
    #[test]
    fn legacy_rigid_body_ancestor_still_owns_its_colliders() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        assert!(has_rigid_body_ancestor(&view, &SdfPath::new("/Mission/LegacyBody/Shell").unwrap()));
        let (has_collider, body) = extract(&view, "/Mission/LegacyBody/Shell");
        assert!(!has_collider, "legacy body's collider child must stay a compound piece");
        assert_eq!(body, None);
    }

    #[test]
    fn rigid_body_ancestry_is_walked_transitively() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        assert!(!has_rigid_body_ancestor(&view, &SdfPath::new("/Mission/Ground").unwrap()));
        assert!(has_rigid_body_ancestor(&view, &SdfPath::new("/Mission/CompoundLander/Hull").unwrap()));
        assert!(!has_rigid_body_ancestor(&view, &SdfPath::new("/Mission/CompoundLander").unwrap()));
    }
}
