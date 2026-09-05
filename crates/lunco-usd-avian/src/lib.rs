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

use avian3d::dynamics::solver::islands::PhysicsIslands;
use avian3d::dynamics::solver::joint_graph::JointGraph;
use avian3d::physics_transform::{Position, Rotation};
use avian3d::prelude::*;
use bevy::ecs::component::ComponentId;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::ecs::system::SystemState;
use bevy::math::{DQuat, DVec3};
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use lunco_core::coords::GridPos;
pub use lunco_usd_bevy::{Purpose, effective_purpose};
use lunco_usd_bevy::{
    ShapeDims, TransformReadError, UsdAnimated, UsdInstanceProjection, UsdPreviewOnly, UsdRead,
    UsdSceneRoot, UsdVisualSynced, instance_key, is_preview_only, local_transform_at,
    read_primitive_axis, read_shape_dims, read_usd_mesh_indexed, usd_axis_to_quat,
};
pub use lunco_usd_bevy::{UsdInstanceRoot, UsdPrimPath, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
// UsdPhysics attribute + API-schema names as CONSTANTS, from openusd's own schema
// module. Hand-written `"physics:…"` string literals are how `physics:friction`
// (an attribute UsdPhysics does not define) got invented and lived here for
// months: a typo in a `&str` compiles.
use openusd::schemas::physics::tokens as ptok;
// `physics:type` is a schema token with a schema enum — take openusd's rather than
// re-spelling `"force"`/`"acceleration"` here.
pub use openusd::schemas::physics::DriveType;

pub mod big_space_bridge;
pub use big_space_bridge::{BigSpacePhysicsBridgePlugin, PhysicsBridgeSystems};

/// Marks an Avian entity synthesized for the currently mounted USD scene.
///
/// Authored physics prims carry [`UsdPrimPath`] and are owned by that stage.
/// Synthesized wheel joints and world-anchor bodies have no authored prim path,
/// so they need this explicit ownership marker for the same teardown transaction
/// to disable and reclaim them. It is not a physics mode or a second lifecycle;
/// it is the ownership fact that the scene boundary cannot infer from hierarchy.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ScenePhysicsOwned;

/// Invalidate the one-shot USD physics projection for a prim whose composed
/// schemas changed after its visual entity was created.
///
/// A live reference can add `PhysicsRigidBodyAPI` to an already-existing
/// instance root. The USD visual projection is then refreshed from the live
/// stage, and this owner-level invalidation lets the Avian observer read the
/// newly composed body contract once more. Physics components are deliberately
/// left intact; the caller only uses this for a prim that was previously
/// typeless and therefore had no Avian body to replace.
pub fn invalidate_usd_physics_projection(world: &mut World, entity: Entity) -> bool {
    if world.get::<RigidBody>(entity).is_some() {
        return false;
    }
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return false;
    };
    entity_mut.remove::<UsdAvianProcessed>();
    true
}

pub mod lint;
pub use lint::{USD_LINT_DOMAIN, physics_facts};

pub mod filtered_pairs;
pub use filtered_pairs::{
    FilteredPairs, PendingFilteredPairs, SharedTireContact, UsdCollisionFilter,
    enable_shared_tire_contact_hooks,
};

pub mod collision_groups;
pub use collision_groups::{CollisionGroupTable, CollisionGroupTables};

/// Bevy plugin for USD physics mapping.
///
/// Adds an observer for USD prim spawning and a deferred processing system that maps
/// USD physics attributes to Avian3D components. The deferred system runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure assets are loaded.
pub struct UsdAvianPlugin;

/// Remove scene physics from Avian's graphs before the scene entities are
/// despawned.
///
/// Avian 0.7 removes contacts when `ColliderMarker` is removed, and removes a
/// joint from its island when its joint component is removed. A raw batch
/// despawn skips those graph transitions long enough for `BodyIslandNode::on_remove`
/// to observe stale constraints. Teardown therefore retires the graph edges
/// directly while the scene bodies are still alive, then removes the ECS
/// components. Going through `JointDisabled` here is unsafe: its observer and
/// the component-removal observer both mutate the same island list during one
/// reload, which can unlink a joint twice.
fn prepare_scene_physics_teardown(world: &mut World) {
    let scene_entities: EntityHashSet = {
        let mut query =
            world.query_filtered::<Entity, Or<(With<UsdPrimPath>, With<ScenePhysicsOwned>)>>();
        query.iter(world).collect()
    };
    let joints: Vec<(Entity, ComponentId)> = {
        let mut query = world.query_filtered::<(
            Entity,
            &avian3d::dynamics::solver::joint_graph::JointComponentId,
        ), (
            With<avian3d::dynamics::solver::joint_graph::JointComponentId>,
            Or<(With<UsdPrimPath>, With<ScenePhysicsOwned>)>,
        )>();
        query
            .iter(world)
            .filter_map(|(entity, joint)| joint.id().map(|id| (entity, id)))
            .collect()
    };
    let colliders: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, (
            With<Collider>,
            Or<(With<UsdPrimPath>, With<ScenePhysicsOwned>)>,
        )>();
        query.iter(world).collect()
    };

    // Retire every graph edge touching this scene, not just joint entities that
    // carry a scene marker. A synthesized constraint may be attached before its
    // ownership marker is visible, while its body is already scene-owned; the
    // body despawn must never be the first graph transition for that edge.
    let graph_joints: Vec<Entity> = world
        .resource::<avian3d::dynamics::solver::joint_graph::JointGraph>()
        .graph()
        .all_edge_weights()
        .filter(|edge| {
            scene_entities.contains(&edge.entity)
                || scene_entities.contains(&edge.body1)
                || scene_entities.contains(&edge.body2)
        })
        .map(|edge| edge.entity)
        .collect();

    // Retire constraints before contacts and bodies. The public graph API lets
    // us tolerate an edge whose island was already emptied by an earlier body
    // teardown without asking Avian's observer to unlink it a second time.
    let mut graph_state: SystemState<(
        ResMut<PhysicsIslands>,
        ResMut<JointGraph>,
        Res<ContactGraph>,
        Query<
            &'static mut avian3d::dynamics::solver::islands::BodyIslandNode,
            Or<(With<Disabled>, Without<Disabled>)>,
        >,
    )> = SystemState::new(world);
    {
        let (mut islands, mut joint_graph, contact_graph, mut body_islands) = graph_state
            .get_mut(world)
            .expect("scene physics teardown parameters must not conflict");

        for entity in graph_joints {
            let Some(edge) = joint_graph.get(entity).cloned() else {
                continue;
            };
            let island_id = edge.island.island_id();
            if island_id != avian3d::dynamics::solver::islands::IslandId::PLACEHOLDER {
                let joint_count = islands
                    .get(island_id)
                    .map(|island| island.joint_count())
                    .unwrap_or(0);
                if joint_count > 0 {
                    let _ = islands.remove_joint(
                        edge.id,
                        &mut body_islands,
                        &contact_graph,
                        &mut joint_graph,
                    );
                }
            }
            joint_graph.remove_joint(entity);
        }
    }
    graph_state.apply(world);

    // Remove the joint component and any marker before despawn. The component
    // removal observer now sees no graph edge, and removing JointComponentId
    // first prevents a JointDisabled removal observer from re-adding anything.
    for (entity, component_id) in joints {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove_by_id(component_id);
            entity_mut.remove::<avian3d::dynamics::solver::joint_graph::JointComponentId>();
            entity_mut.remove::<JointDisabled>();
        }
    }

    // Removing ColliderMarker is Avian's supported contact-graph removal path;
    // removing only Collider leaves its required marker alive until too late.
    for entity in colliders {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ColliderMarker>();
        }
    }
}

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        // Installs joints parked by `attach_joint` — the USD path attaches
        // authored joints, so this app must be able to land them.
        app.add_plugins(JointAttachPlugin);
        app.add_systems(lunco_core::SceneTeardown, prepare_scene_physics_teardown);
        // `on_add_usd_prim`: eager observer for joint pending-state.
        // `process_usd_avian_prims`: observer on UsdVisualSynced — fires
        //   right after the USD structural projection translates each prim,
        //   so the stage and Transform exist. CPU visual meshes may still be
        //   streaming; mesh-backed terrain has its own pending-collider phase.
        // `build_usd_physics_joints`: stays a per-frame system because
        //   it's a deferred state-machine waiting for both referenced bodies
        //   and their bridge-seeded poses.
        //   `run_if(any pending)` makes it idle when no joints await.
        // `PhysicsSceneGravity` records which prim set the world's gravity, which
        // is only meaningful while that scene is loaded — carried into the next
        // scene it would make a fresh `PhysicsScene` look like a conflicting
        // duplicate of a prim that no longer exists.
        // Findings name prims of the scene being replaced, so they go with it.
        // Note there is NO automatic lint on load: linting is something you RUN
        // (`RunLint`), not something that runs at you — see `lunco-lint`.
        app.init_resource::<lunco_lint::LintReport>();
        app.init_resource::<CollisionGroupTables>();
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut commands: Commands,
             mut lint: ResMut<lunco_lint::LintReport>,
             mut groups: ResMut<CollisionGroupTables>| {
                commands.remove_resource::<lunco_environment::PhysicsSceneGravity>();
                lint.clear_domain(lint::USD_LINT_DOMAIN);
                // The groups belong to the scene being replaced. Carried over,
                // they would put the next scene's colliders on layers nothing in
                // it defines.
                groups.clear();
            },
        );

        app.register_type::<ShouldBeDynamic>()
            .register_type::<filtered_pairs::SharedTireContact>()
            .register_type::<lunco_core::Mobility>()
            .add_observer(on_add_usd_prim)
            .add_observer(process_usd_avian_prims)
            // The joint builder is preparation, not integration. It runs in the
            // enclosing fixed schedule after the bridge's hold-safe read pass,
            // so scene readiness can resolve authored joints even while
            // `Time<Physics>` is paused. The outer Update admission pass runs
            // after the deferred commands are flushed; the next fixed physics
            // step then consumes the admitted constraint after solver bodies
            // exist.
            .add_systems(
                FixedPostUpdate,
                (
                    build_usd_physics_joints
                        .in_set(avian3d::prelude::PhysicsSystems::Prepare)
                        .after(big_space_bridge::PhysicsBridgeSystems::Read)
                        .after(
                            avian3d::dynamics::rigid_body::mass_properties::MassPropertySystems::UpdateComputedMassProperties,
                        )
                        .run_if(any_with_component::<PendingUsdJoint>),
                    bevy::ecs::schedule::ApplyDeferred,
                )
                    .chain()
                    .before(avian3d::prelude::PhysicsSystems::StepSimulation),
            )
            .add_systems(
                avian3d::schedule::PhysicsSchedule,
                filtered_pairs::resolve_filtered_pairs
                    .run_if(any_with_component::<PendingFilteredPairs>)
                    .in_set(avian3d::prelude::PhysicsSystems::Prepare)
                    .after(avian3d::prelude::PhysicsSystems::First)
                    .before(avian3d::schedule::PhysicsStepSystems::First),
            )
            .add_systems(
                Update,
                (
                    build_terrain_mesh_colliders
                        .run_if(any_with_component::<PendingTerrainCollider>),
                    enforce_kinematic_on_animated,
                    filtered_pairs::enable_shared_tire_contact_hooks,
                    filtered_pairs::enable_static_friction_contact_hooks,
                    filtered_pairs::synchronize_collision_hook_flags
                        .after(filtered_pairs::enable_static_friction_contact_hooks),
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
    q: Query<(Entity, &lunco_core::Mobility), (Changed<lunco_core::Mobility>, Without<RigidBody>)>,
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

        assert_eq!(
            app.world().get::<RigidBody>(bare),
            Some(&RigidBody::Dynamic)
        );
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
        (
            With<UsdAnimated>,
            Or<(Added<RigidBody>, Added<UsdAnimated>)>,
        ),
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
    /// Basis of the joint frame on body0, in body0's local frame (UsdPhysics
    /// `physics:localRot0`). Identity when unauthored. [`axis`](Self::axis) is
    /// read IN this basis, and every joint but the spherical constrains
    /// `rot0 · local_rot0` to `rot1 · local_rot1` — so a pair of bodies that rest
    /// at different orientations is expressed here, not by tilting the axis.
    pub local_rot0: DQuat,
    /// Basis of the joint frame on body1 (UsdPhysics `physics:localRot1`).
    /// Identity when unauthored. See [`local_rot0`](Self::local_rot0).
    pub local_rot1: DQuat,
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
    /// Passive relative-velocity damping explicitly classified by
    /// `LunCoJointDampingAPI`, or `None` when the joint is lossless. This maps
    /// directly to Avian's native `JointDamping`; it is not a drive or a
    /// rigid-body world-damping approximation.
    pub damping: Option<JointDamping>,
}

/// The `UsdPhysicsJoint` base reads every joint type shares: the two bodies and
/// the two joint frames (`physics:localPos0/1` + `physics:localRot0/1`).
struct JointBaseRead {
    body0: String,
    body1: String,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
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
    /// `drive:{angular,linear}:physics:stiffness` — N/m (linear) or N·m/rad
    /// (angular).
    pub stiffness: Option<f64>,
    /// `drive:{angular,linear}:physics:damping` — N·s/m (linear) or N·m·s/rad
    /// (angular).
    pub damping: Option<f64>,
    /// `drive:{angular,linear}:physics:type` — whether the coefficients above
    /// produce a force directly or an acceleration the solver scales by mass.
    /// `None` = unauthored, and the schema's own fallback for that is
    /// [`DriveType::Force`].
    pub drive_type: Option<DriveType>,
    /// The authored generalized inertia of the driven coordinate: kilograms for
    /// a linear drive, or kg·m² for an angular drive. A force-type spring is
    /// realised as a stable [`MotorModel::SpringDamper`], whose frequency is
    /// `sqrt(stiffness / generalized_inertia)`; see [`JointDrive::motor_model`].
    /// `None` means USD left the value to Avian's computed mass-property path;
    /// runtime joint construction resolves it from the participating bodies'
    /// attached geometry, density, mass and inertia.
    pub generalized_inertia: Option<f64>,
}

impl JointDrive {
    /// The avian motor model this drive asks for.
    ///
    /// `UsdPhysicsDriveAPI` defines the drive law —
    /// `force = stiffness * (targetPosition - position) + damping * (targetVelocity -
    /// velocity)` — and its one axis of variation, `physics:type`: `"force"` applies
    /// that as a force, `"acceleration"` applies it mass-normalised so the response
    /// does not depend on what the joint is carrying.
    ///
    /// `"acceleration"` maps straight onto [`MotorModel::AccelerationBased`], same
    /// coefficients, same units. `"force"` does NOT map onto
    /// [`MotorModel::ForceBased`]: avian's `ForceBased` is an EXPLICIT integrator
    /// that is unstable for a stiff, damped drive on a heavy body at a sim tick —
    /// the landing leg (k = 4000 N/m, c = 2200 N·s/m, m = 500 kg, 60 Hz) freezes at
    /// its rest offset and never bears load. avian's own docs say to use
    /// [`MotorModel::SpringDamper`] (the IMPLICIT form) for stability.
    ///
    /// SpringDamper is parameterised by `frequency` and `damping_ratio`, and
    /// `omega = sqrt(stiffness / mass)`, `zeta = damping / (2*sqrt(stiffness*mass))`
    /// recover EXACTLY the authored law: SpringDamper's per-substep correction is an
    /// acceleration `omega^2*pos_err + 2*zeta*omega*vel_err`, so the force it
    /// develops is `mass * that = stiffness*pos_err + damping*vel_err` — `force =
    /// k*x + c*v`, unchanged, but integrated stably. The conversion needs the driven
    /// coordinate's generalized inertia ([`Self::generalized_inertia`]), which is
    /// the mass for a LINEAR drive and the effective moment about the hinge for an
    /// ANGULAR drive; the runtime resolver supplies Avian's computed value when
    /// USD left it unauthored. If neither authored nor computed properties can
    /// certify it, the drive is rejected rather than silently reverting to an
    /// uncertified explicit motor.
    ///
    /// An unauthored `physics:type` takes the schema's own fallback, `"force"`
    /// (`usdPhysics` declares `uniform token physics:type = "force"`).
    ///
    /// A drive with neither coefficient is not a spring but a positioner: it seeks a
    /// setpoint, and how fast it converges is a tuning choice rather than a property
    /// of the mechanism. That one gets [`MotorModel::SpringDamper`] at a fixed
    /// frequency, which is unconditionally stable under XPBD substepping at any mass.
    fn motor_model(&self) -> Result<MotorModel, lunco_physics::ForceDriveMotorError> {
        if self.stiffness.is_none() && self.damping.is_none() {
            return Ok(JOINT_DRIVE_MOTOR_MODEL);
        }
        let stiffness = self.stiffness.unwrap_or(0.0);
        let damping = self.damping.unwrap_or(0.0);
        if stiffness == 0.0 && damping == 0.0 {
            return Ok(JOINT_DRIVE_MOTOR_MODEL);
        }
        match self.drive_type.unwrap_or(DriveType::Force) {
            DriveType::Acceleration => Ok(MotorModel::AccelerationBased { stiffness, damping }),
            DriveType::Force => lunco_physics::force_drive_motor_model(
                stiffness,
                damping,
                self.generalized_inertia.unwrap_or(0.0),
            ),
        }
    }

    /// Whether the motor should start enabled.
    ///
    /// A spring IS an active motor whose target is its own rest position, so a
    /// drive with `targetPosition` left at its default is still live: it must push
    /// back the moment the joint leaves that rest offset. Activation therefore
    /// keys on authored stiffness/damping as well as on targets, never on a
    /// setpoint alone.
    pub fn is_active(&self) -> bool {
        self.target_position.is_some()
            || self.target_velocity.is_some()
            || self.stiffness.is_some()
            || self.damping.is_some()
    }
}

/// Mass properties needed to turn a USD force drive into Avian's implicit
/// spring-damper model. These are the live, composed properties after Avian has
/// combined the body's own collider tree and any authored mass overrides.
#[derive(Clone, Copy)]
struct LiveDriveMassProperties {
    mass: f64,
    angular_inertia: ComputedAngularInertia,
    center_of_mass: DVec3,
}

/// Why a live body's computed mass properties cannot yet be used by a drive.
#[derive(Clone, Copy)]
enum LiveDriveMassPropertiesError {
    /// Avian has not run its mass-property update for this body yet.
    NotReady,
    /// Avian has produced an infinite/degenerate property for a body that the
    /// drive expects to move.
    Invalid,
}

/// The result of resolving a force drive's generalized inertia. `Waiting` is a
/// real lifecycle state: USD permits mass/inertia to be omitted, and Avian
/// computes them from attached colliders. The joint must wait for that computed
/// state rather than rejecting a valid stage or installing a timestep-sensitive
/// explicit motor.
enum ResolvedJointDrive {
    Ready(MotorModel),
    Waiting,
    Invalid(lunco_physics::ForceDriveMotorError),
}

/// Return the finite live mass properties for an endpoint.
///
/// `None` is a world/static endpoint and therefore contributes infinite
/// generalized inertia. An `Err` is deliberately distinct from that case: a
/// dynamic/kinematic body whose computed properties are not available yet must
/// defer joint admission, while a body whose computed properties are present but
/// degenerate is a terminal physics authoring error.
fn live_drive_mass_properties(
    query: &Query<(
        &RigidBody,
        Option<&ShouldBeDynamic>,
        Option<&ComputedMass>,
        Option<&ComputedAngularInertia>,
        Option<&ComputedCenterOfMass>,
    )>,
    entity: Option<Entity>,
) -> Result<Option<LiveDriveMassProperties>, LiveDriveMassPropertiesError> {
    let Some(entity) = entity else {
        return Ok(None);
    };
    let Ok((body, should_be_dynamic, mass, angular_inertia, center_of_mass)) = query.get(entity)
    else {
        return Err(LiveDriveMassPropertiesError::NotReady);
    };
    if matches!(body, RigidBody::Static)
        || (matches!(body, RigidBody::Kinematic) && should_be_dynamic.is_none())
    {
        return Ok(None);
    }
    let (Some(mass), Some(angular_inertia), Some(center_of_mass)) =
        (mass, angular_inertia, center_of_mass)
    else {
        return Err(LiveDriveMassPropertiesError::NotReady);
    };
    let mass = mass.value();
    let center_of_mass = center_of_mass.0;
    if !mass.is_finite()
        || mass <= 0.0
        || !angular_inertia.is_finite()
        || !center_of_mass.is_finite()
    {
        return Err(LiveDriveMassPropertiesError::Invalid);
    }
    Ok(Some(LiveDriveMassProperties {
        mass,
        angular_inertia: *angular_inertia,
        center_of_mass,
    }))
}

/// Resolve a USD force drive against Avian's computed attached-body properties.
///
/// Authored generalized inertia wins when present. When it is absent, the
/// implicit conversion is calculated from the actual live bodies: effective
/// mass for a slider, or effective moment about the joint axis for a hinge,
/// including the parallel-axis term from each body's computed centre of mass.
/// This is the general articulated-body path; it contains no rover or steering
/// knowledge.
fn resolve_joint_drive_motor_model(
    drive: JointDrive,
    pending: &PendingUsdJoint,
    body0: Option<Entity>,
    body1: Option<Entity>,
    pose0: Option<(DVec3, DQuat)>,
    pose1: Option<(DVec3, DQuat)>,
    mass_properties: &Query<(
        &RigidBody,
        Option<&ShouldBeDynamic>,
        Option<&ComputedMass>,
        Option<&ComputedAngularInertia>,
        Option<&ComputedCenterOfMass>,
    )>,
) -> ResolvedJointDrive {
    // This first call validates the authored coefficients and resolves all
    // acceleration drives and force drives that do not need an inertia. Only a
    // missing generalized inertia proceeds to live-property derivation.
    match drive.motor_model() {
        Ok(model) => return ResolvedJointDrive::Ready(model),
        Err(lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia) => {}
        Err(error) => return ResolvedJointDrive::Invalid(error),
    }

    let properties0 = match live_drive_mass_properties(mass_properties, body0) {
        Ok(properties) => properties,
        Err(LiveDriveMassPropertiesError::NotReady) => return ResolvedJointDrive::Waiting,
        Err(LiveDriveMassPropertiesError::Invalid) => {
            return ResolvedJointDrive::Invalid(
                lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia,
            );
        }
    };
    let properties1 = match live_drive_mass_properties(mass_properties, body1) {
        Ok(properties) => properties,
        Err(LiveDriveMassPropertiesError::NotReady) => return ResolvedJointDrive::Waiting,
        Err(LiveDriveMassPropertiesError::Invalid) => {
            return ResolvedJointDrive::Invalid(
                lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia,
            );
        }
    };

    let generalized_inertia = if pending.joint_type == "PhysicsPrismaticJoint" {
        let inverse_mass = properties0.map(|p| 1.0 / p.mass).unwrap_or(0.0)
            + properties1.map(|p| 1.0 / p.mass).unwrap_or(0.0);
        if !inverse_mass.is_finite() || inverse_mass <= f64::EPSILON {
            return ResolvedJointDrive::Invalid(
                lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia,
            );
        }
        1.0 / inverse_mass
    } else if pending.joint_type == "PhysicsRevoluteJoint" {
        let scalar_inertia = |properties: Option<LiveDriveMassProperties>,
                              pose: Option<(DVec3, DQuat)>,
                              local_axis: DVec3,
                              local_anchor: DVec3|
         -> Result<Option<f64>, ResolvedJointDrive> {
            let Some(properties) = properties else {
                return Ok(None);
            };
            let Some((position, rotation)) = pose else {
                return Err(ResolvedJointDrive::Waiting);
            };
            let local_axis = local_axis.normalize_or_zero();
            if !local_axis.is_finite() || local_axis.length_squared() <= f64::EPSILON {
                return Err(ResolvedJointDrive::Invalid(
                    lunco_physics::ForceDriveMotorError::InvalidCoefficients,
                ));
            }
            let rotational = local_axis.dot(properties.angular_inertia.value() * local_axis);
            let axis_world = rotation * local_axis;
            let anchor_world = position + rotation * local_anchor;
            let center_of_mass_world = position + rotation * properties.center_of_mass;
            let offset = anchor_world - center_of_mass_world;
            let perpendicular_offset_squared =
                (offset.length_squared() - offset.dot(axis_world).powi(2)).max(0.0);
            let scalar = rotational + properties.mass * perpendicular_offset_squared;
            if !scalar.is_finite() || scalar <= f64::EPSILON {
                return Err(ResolvedJointDrive::Invalid(
                    lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia,
                ));
            }
            Ok(Some(scalar))
        };
        let i0 = match scalar_inertia(
            properties0,
            pose0,
            pending.local_rot0 * pending.axis,
            pending.local_pos0,
        ) {
            Ok(value) => value,
            Err(result) => return result,
        };
        let i1 = match scalar_inertia(
            properties1,
            pose1,
            pending.local_rot1 * pending.axis,
            pending.local_pos1,
        ) {
            Ok(value) => value,
            Err(result) => return result,
        };
        let inverse = i0.map(|i| 1.0 / i).unwrap_or(0.0) + i1.map(|i| 1.0 / i).unwrap_or(0.0);
        if !inverse.is_finite() || inverse <= f64::EPSILON {
            return ResolvedJointDrive::Invalid(
                lunco_physics::ForceDriveMotorError::MissingGeneralizedInertia,
            );
        }
        1.0 / inverse
    } else {
        // Fixed/spherical/distance joints do not install a linear/angular motor
        // from this reader. Keep the resolution closed over the explicit joint
        // kinds that carry the corresponding USD drive instance.
        return ResolvedJointDrive::Invalid(
            lunco_physics::ForceDriveMotorError::InvalidCoefficients,
        );
    };

    let mut resolved = drive;
    resolved.generalized_inertia = Some(generalized_inertia);
    match resolved.motor_model() {
        Ok(model) => ResolvedJointDrive::Ready(model),
        Err(error) => ResolvedJointDrive::Invalid(error),
    }
}

/// Overdamped spring-damper for a joint drive that authors no stiffness or
/// damping — mirrors `lunco_cosim::joint`'s motor model so a USD-driven joint and
/// a wire-driven one track their setpoint identically (≈3 Hz, ζ=2, no overshoot
/// under XPBD substepping). A drive that DOES author them is a physical spring;
/// see [`JointDrive::motor_model`].
const JOINT_DRIVE_MOTOR_MODEL: MotorModel = MotorModel::SpringDamper {
    frequency: 3.0,
    damping_ratio: 2.0,
};

/// Force (N) / torque (N·m) saturation a USD-driven joint motor gets when its
/// `physics:maxForce` is left unauthored — generous enough to hold the target
/// against gravity, matching `lunco_cosim::joint`'s wire-driven default.
const JOINT_DRIVE_MAX_FORCE_DEFAULT: f64 = 1.0e8;

/// Checks if a USD prim has a specific API schema applied.
/// Collects collider shapes from all descendant prims of a compound body root,
/// reading directly from the USD stage.
///
/// Returns a list of `(Position, Rotation, Collider)` tuples for `Collider::compound()`.
fn collect_child_colliders_from_usd(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    parent_path: &SdfPath,
) -> Result<Vec<(Position, Rotation, Collider)>, TransformReadError> {
    let mut shapes = Vec::new();
    let convention = lunco_usd_bevy::stage_convention(reader).map_err(|_| TransformReadError {
        prim: parent_path.as_str().to_owned(),
    })?;

    // Per the spec a rigid body aggregates ALL descendant colliders, not only
    // direct children — a collider under an intermediate grouping `Xform`
    // (`/Rover/Shapes/Hull`) is still this body's geometry. Descendant walk with
    // each prim's transform composed in the root's frame; recursion stops at a
    // nested-body boundary (see `gather_compound_candidates`).
    let mut candidates = Vec::new();
    gather_compound_candidates(reader, parent_path, Transform::IDENTITY, &mut candidates)?;
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
        if let Some(collider) = build_collider_from_usd_at_scale(reader, &child_path, scale) {
            let pos = Position(DVec3::new(
                child_tf.translation.x as f64,
                child_tf.translation.y as f64,
                child_tf.translation.z as f64,
            ));
            let rot = Rotation(child_tf.rotation.as_dquat());
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
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
fn build_collider_from_usd(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<Option<Collider>, TransformReadError> {
    let scale =
        local_transform_at(reader, sdf_path, 0.0)?.map_or(Vec3::ONE, |transform| transform.scale);
    Ok(build_collider_from_usd_at_scale(reader, sdf_path, scale))
}

/// Build a collider with a scale already composed from the owning body frame to
/// the geometry prim. Standalone colliders obtain this from their own local
/// transform; compound children obtain it from [`gather_compound_candidates`],
/// because intermediate USD Xforms have no corresponding Avian collider entity.
fn build_collider_from_usd_at_scale(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
            None | Some("none") => Collider::trimesh(verts, tris),
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

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(), TransformReadError> {
    if let Some(collider) = build_collider_from_usd(reader, sdf_path)? {
        commands.entity(entity).try_insert(collider);
    }
    Ok(())
}

fn log_malformed_collider_transform(sdf_path: &SdfPath, error: &TransformReadError) {
    error!(
        "[usd-avian] {sdf_path} has malformed collider transform; refusing collider projection: {error}"
    );
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
fn has_rigid_body_ancestor(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> bool {
    let mut cur = sdf_path.parent();
    while let Some(p) = cur {
        if p.is_abs_root() {
            return false;
        }
        if reader.has_api_schema(&p, ptok::API_RIGID_BODY) {
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
fn is_avian_body(reader: &dyn lunco_usd_bevy::read::UsdReadObject, path: &SdfPath) -> bool {
    reader.has_api_schema(path, ptok::API_RIGID_BODY)
        || reader.has_api_schema(path, "LunCoTerrainAPI")
        || (reader.has_api_schema(path, ptok::API_COLLISION)
            && !has_rigid_body_ancestor(reader, path))
}

/// The body a joint endpoint actually attaches to: `path` itself when it is a
/// body, otherwise its NEAREST ANCESTOR that is one.
///
/// **Why an endpoint may name a non-body.** A mechanism that mounts on something
/// — an antenna on a rover, a lander, a tower — has to name the thing it mounts
/// to. If that must be the HOST's body prim, the component is naming a path it
/// cannot know, so every host ends up reaching into the component's namespace and
/// authoring the mount joint itself. That is exactly what happened: `AntennaYawJoint`
/// was written three times, in three hosts, each targeting a prim inside a nested
/// reference. With this rule the component names its OWN root, and parenting it
/// under a vehicle is the mount.
///
/// The rule keys off "a named prim that is not a body". It never keys off an
/// EMPTY rel — UsdPhysics already gives that the meaning "world". `None` when
/// the path names nothing that is or sits under a
/// body, which stays an unresolved joint and still warns.
///
/// Resolving here rather than at ECS-match time is deliberate: `read_joint_spec`
/// derives an unauthored anchor from the two body paths ([`derive_joint_anchor`]),
/// and that derivation must run against the frame of the body the joint is really
/// built on. Resolving later would leave the anchor expressed in the wrong frame.
fn nearest_body_path(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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

/// Resolve a USD joint relationship target to the rigid-body prim that owns the
/// endpoint. The relationship may name a mechanism child inside a referenced
/// component; the joint contract attaches to that child's nearest body ancestor.
/// Keep this resolution in the Avian USD reader so topology consumers cannot
/// accidentally compare an unresolved authored path with a resolved ECS path.
pub fn resolve_joint_body_path(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    target: &str,
) -> Option<String> {
    let path = SdfPath::new(target).ok()?;
    nearest_body_path(reader, &path).map(|resolved| resolved.to_string())
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
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            continue;
        };

        let collider = heightfield_from_mesh(mesh).or_else(|| {
            warn!(
                "[usd-avian] terrain mesh isn't a regular DEM grid; \
                   building a (heavier) trimesh collider instead"
            );
            Collider::trimesh_from_mesh(mesh)
        });

        match collider {
            Some(c) => {
                info!(
                    "[usd-avian] terrain collider built ({} verts)",
                    mesh.count_vertices()
                );
                commands
                    .entity(entity)
                    .try_insert(c)
                    .remove::<PendingTerrainCollider>();
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
    let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
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
    let row_const_z =
        (pos[0][2] - pos[1][2]).abs() < eps && (pos[0][2] - pos[side - 1][2]).abs() < eps;
    let col_const_x = (pos[0][0] - pos[side][0]).abs() < eps;
    if !row_const_z || !col_const_x {
        return None;
    }

    let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
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

    Some(Collider::heightfield(
        heights,
        DVec3::new(scale_x, 1.0, scale_z),
    ))
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
/// Observer: fires once per entity, the moment the USD structural projection
/// translates the prim (signalled by inserting `UsdVisualSynced`). CPU visual
/// meshes may still be streaming; physics reads the worker-produced composed
/// projection plan and does not depend on `Mesh3d` or a live `Stage`.
/// By that point the plan is committed and the same reader contract used by
/// visual and simulation projection is available for physics components.
fn process_usd_avian_prims(
    trigger: On<Add, UsdVisualSynced>,
    query: Query<(&UsdPrimPath, Option<&UsdInstanceProjection>), Without<UsdAvianProcessed>>,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    mount_state: Option<Res<lunco_core::SceneMountState>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy::CanonicalStages>,
    mut group_tables: ResMut<CollisionGroupTables>,
    mut commands: Commands,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    let entity = trigger.entity;
    let Ok((prim_path, instance_projection)) = query.get(entity) else {
        return;
    };
    // A USD Editor preview shares the normal visual projection pipeline with
    // the live scene, but it is not a second physical world.  The preview root
    // is deliberately marked `UsdPreviewOnly`; walk to it before reading any
    // PhysicsRigidBodyAPI so Avian cannot admit a duplicate `/Griffin1` body or
    // a duplicate joint/collider graph into the simulation.
    if is_preview_only(entity, &q_child_of, &q_preview_only) {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }
    if let Some(mount_state) = mount_state {
        let stale_mount = match lunco_usd_bevy::scene_root_ancestor(
            entity,
            &q_scene_root,
            &q_child_of,
            &q_entities,
        ) {
            Ok(Some(root)) => !mount_state.contains_root(root),
            Ok(None) => false,
            Err(_) => true,
        };
        if stale_mount {
            // `UsdVisualSynced` can be applied from a command buffer that was
            // filled before a scene replacement request.  Do not let this
            // observer admit a collider/body into a root whose teardown is
            // already owned by the newer transaction; Avian's collider
            // observer would otherwise enqueue a non-fallible AncestorMarker
            // insert and panic when the old root is applied away.
            return;
        }
    }
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
        return;
    };

    let id = prim_path.stage_handle.id();
    let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
        return;
    };
    let (reader, _generation) = canonical.reader_for_entity(id, stage_asset, instance_projection);
    bevy::log::debug!(
        "[canonical] avian extract from composed reader: {}",
        prim_path.path
    );

    // Collision groups are a STAGE-wide statement read one prim at a time, so the
    // table is resolved once per stage and cached; recomputing it per prim would
    // be quadratic in prim count on a scene that authors any group at all.
    let groups = group_tables.get_or_read(id, &reader).clone();
    extract_avian_prim(
        &reader,
        entity,
        &sdf_path,
        &groups,
        &mut commands,
        faults.as_deref_mut(),
        holds.as_deref_mut(),
    );
}

/// Set the world's gravity from a composed `UsdPhysicsScene` prim.
///
/// `physics:gravityMagnitude` is in scene units per second squared and
/// `physics:gravityDirection` is a vector in the STAGE's frame, so both convert
/// at this boundary like every other authored quantity — magnitude by
/// `metersPerUnit`, direction by the up-axis convention.
///
/// UsdPhysics gives each attribute a "use the default" sentinel rather than
/// leaving it unauthored: a NEGATIVE magnitude means "earth gravity", and a ZERO
/// direction vector means "the stage's down axis". Honouring both is what lets a
/// scene author only the half it cares about — a lunar scene names 1.62 and says
/// nothing about direction.
fn read_physics_scene_gravity(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(f64, DVec3), &'static str> {
    let convention = lunco_usd_bevy::stage_convention(reader)
        .map_err(|_| "stage convention metadata is invalid")?;
    let magnitude = match reader.real(sdf_path, ptok::A_GRAVITY_MAGNITUDE) {
        Some(value) if value < 0.0 => lunco_environment::EARTH_SURFACE_GRAVITY,
        Some(value) if value.is_finite() => {
            let converted = convention.length(value);
            if !converted.is_finite() {
                return Err("gravity magnitude is not finite after stage-unit conversion");
            }
            converted
        }
        Some(_) => return Err("gravity magnitude must be finite or negative for Earth gravity"),
        None if !reader.has_authored_attribute(sdf_path, ptok::A_GRAVITY_MAGNITUDE) => {
            lunco_environment::EARTH_SURFACE_GRAVITY
        }
        None => return Err("gravity magnitude has an unsupported authored value type"),
    };
    let raw_direction = match reader.vec3_f64(sdf_path, ptok::A_GRAVITY_DIRECTION) {
        Some(value) => DVec3::from_array(value),
        None if !reader.has_authored_attribute(sdf_path, ptok::A_GRAVITY_DIRECTION) => DVec3::ZERO,
        None => return Err("gravity direction has an unsupported authored value type"),
    };
    if !raw_direction.is_finite() {
        return Err("gravity direction must be finite");
    }
    let direction = if raw_direction == DVec3::ZERO {
        DVec3::NEG_Y
    } else {
        convention
            .dir_d(raw_direction)
            .try_normalize()
            .ok_or("gravity direction must not be degenerate")?
    };
    Ok((magnitude, direction))
}

fn apply_physics_scene_gravity(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
    commands: &mut Commands,
) {
    let (magnitude, direction) = match read_physics_scene_gravity(reader, sdf_path) {
        Ok(values) => values,
        Err(reason) => {
            error!(
                "[usd-avian] {sdf_path} has malformed PhysicsScene gravity data ({reason}); refusing gravity projection"
            );
            return;
        }
    };

    let prim = sdf_path.as_str().to_string();
    commands.queue(move |world: &mut World| {
        // A scene has ONE gravity, so two `PhysicsScene` prims that disagree are
        // an authoring error. Report it, then apply anyway: the write must be
        // unconditional or a scene RELOAD — whose prims are a different set from
        // the outgoing scene's — would be refused its own gravity and inherit
        // the previous scene's.
        if let Some(existing) = world.get_resource::<lunco_environment::PhysicsSceneGravity>() {
            let disagrees = (existing.magnitude - magnitude).abs() > 1e-9
                || !existing.direction.abs_diff_eq(direction, 1e-9);
            if existing.prim != prim && disagrees {
                error!(
                    "[usd-avian] two PhysicsScene prims disagree about gravity: `{}` set \
                     {:.4} m/s² along {:?}, `{}` sets {:.4} m/s² along {:?}. The last one \
                     read wins, which depends on prim order — a scene has one gravity, so \
                     author a single PhysicsScene.",
                    existing.prim,
                    existing.magnitude,
                    existing.direction,
                    prim,
                    magnitude,
                    direction
                );
            }
        }
        info!("[usd-avian] {prim} sets gravity to {magnitude:.4} m/s² along {direction:?}");
        world.insert_resource(lunco_environment::Gravity::flat(magnitude, direction));
        world.insert_resource(lunco_environment::PhysicsSceneGravity {
            prim,
            magnitude,
            direction,
        });
    });
}

/// Map a single composed USD prim to its Avian physics components through the
/// shared reader boundary. Split out of the observer so the read body can be
/// driven directly by tests.
fn is_physics_joint_type(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> bool {
    matches!(
        reader.type_name(sdf_path).as_deref(),
        Some(
            ptok::T_PHYSICS_JOINT
                | ptok::T_PHYSICS_FIXED_JOINT
                | ptok::T_PHYSICS_REVOLUTE_JOINT
                | ptok::T_PHYSICS_PRISMATIC_JOINT
                | ptok::T_PHYSICS_SPHERICAL_JOINT
                | ptok::T_PHYSICS_DISTANCE_JOINT
        )
    )
}

/// Project a standard USD joint from the shared composed-prim boundary.
///
/// Both the loaded-stage observer and the visual-admission path call this
/// function. The latter is required for runtime-spawned assets whose USD handle
/// can arrive before the stage. This keeps valid joints and malformed-joint
/// faults independent of loading order.
fn project_pending_joint(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    entity: Entity,
    sdf_path: &SdfPath,
    commands: &mut Commands,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
) -> bool {
    if !is_physics_joint_type(reader, sdf_path) {
        return false;
    }
    // Wheel joints belong to raycast-wheel realization. Lint-only fixtures are
    // available to the linter but are never runtime constraints or faults.
    if reader.boolean(sdf_path, "lunco:lintOnly") == Some(true)
        || joint_targets_simulated_wheel(reader, sdf_path)
    {
        return true;
    }
    if let Some(joint) = read_joint_spec(reader, sdf_path) {
        commands
            .entity(entity)
            .try_insert((joint, lunco_physics::PhysicsJointPending));
    } else if reader.boolean(sdf_path, ptok::A_JOINT_ENABLED) != Some(false) {
        let detail = "standard UsdPhysics joint was not projected: invalid body relationship, frame, axis, limit, or drive authoring";
        error!("USD physics joint {} rejected: {detail}", sdf_path);
        if let Some(faults) = faults {
            faults.raise(
                "usd-physics-joint-invalid",
                Some(entity),
                sdf_path.to_string(),
                detail,
            );
        }
        if let Some(holds) = holds {
            holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        }
    }
    true
}

fn extract_avian_prim(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    entity: Entity,
    sdf_path: &SdfPath,
    groups: &CollisionGroupTable,
    commands: &mut Commands,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
) {
    // Joint projection is owned by the same composed-prim boundary as every
    // other Avian projection. The legacy `Add<UsdPrimPath>` observer can run
    // before its stage is available; relying on it alone lets terrain consume
    // a support request before the joint topology exists. A joint is a
    // constraint declaration, not a body/collider, so finish this prim here
    // and leave native admission to the shared pending-joint path.
    if project_pending_joint(reader, entity, sdf_path, commands, faults, holds) {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    // `UsdPhysicsScene` — simulation-wide settings, of which this engine consumes
    // gravity. It is a SETTINGS prim, not a body: it has no transform and no
    // collider, so it is handled here and the body/collider reads below are
    // skipped entirely.
    if reader.type_name(sdf_path).as_deref() == Some(ptok::T_PHYSICS_SCENE) {
        apply_physics_scene_gravity(reader, sdf_path, commands);
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    // `guide` geometry is annotation — a debug axis, a planned path, a sensor
    // cone. It is never physical, whatever schemas happen to be on it, so it is
    // refused a body and a collider both rather than being quietly collided with.
    if effective_purpose(reader, sdf_path) == Purpose::Guide {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    // `PhysicsFilteredPairsAPI` applies to a body OR to a collider under one, and
    // is read before either branch below because it is orthogonal to both: it says
    // which pairs never collide, not what this prim IS.
    if let Some(pending) = filtered_pairs::read_filtered_pairs(reader, sdf_path) {
        commands.entity(entity).try_insert(pending);
    }

    // Skip wheel prims — the sim plugin handles their colliders and bodies. The
    // standard filtered-pairs API above is still owned by this bridge, because
    // it is orthogonal to wheel realization and must be admitted before the
    // wheel projector returns.
    if reader
        .real_f32(sdf_path, "physxVehicleWheel:radius")
        .is_some()
    {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    let has_rigid_body_api = reader.has_api_schema(sdf_path, ptok::API_RIGID_BODY);
    let has_collision_api = reader.has_api_schema(sdf_path, ptok::API_COLLISION);
    let has_terrain_api = reader.has_api_schema(sdf_path, "LunCoTerrainAPI");
    // ── TERRAIN ── static collider + TerrainTile; mesh DEMs defer their collider.
    if has_terrain_api {
        if apply_physics_material(commands, entity, reader, sdf_path).is_err() {
            error!(
                "[usd-avian] {sdf_path} has malformed physics-material values — refusing terrain projection"
            );
            commands.entity(entity).try_insert(UsdAvianProcessed);
            return;
        }
        commands.entity(entity).try_insert((
            RigidBody::Static,
            lunco_core::Mobility::Static,
            lunco_terrain_globe::TerrainTile,
        ));
        // Terrain is a static body, but it is still a USD physics surface.
        // Apply the authored material before its collider is admitted so the
        // solver combines the ground's friction/restitution with the touching
        // body's material exactly as it does for a dynamic body.  Keeping this
        // on the classification branch avoids a scene-specific ground override.
        match build_collider_from_usd(reader, sdf_path) {
            Ok(Some(collider)) => {
                commands.entity(entity).try_insert(collider);
            }
            Ok(None) => {
                commands.entity(entity).try_insert(PendingTerrainCollider);
            }
            Err(error) => {
                log_malformed_collider_transform(sdf_path, &error);
            }
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    // ── TRIGGER ZONE ── `lunco:triggerZone` → overlap-only static Sensor.
    if let Some(zone) = reader
        .text(sdf_path, "lunco:triggerZone")
        .filter(|z| !z.trim().is_empty())
    {
        commands
            .entity(entity)
            .try_insert((RigidBody::Static, lunco_core::Mobility::Static));
        // Avian snapshots `CollisionEventsEnabled` when the collider proxy is
        // inserted into its tree. Insert the complete trigger contract first;
        // adding the event marker after the collider leaves an already-created
        // contact edge with CONTACT_EVENTS=false, so no CollisionStart message
        // can ever be emitted for a waypoint that was present from frame zero.
        commands.entity(entity).try_insert((
            Sensor,
            CollisionEventsEnabled,
            lunco_core::TriggerZone(zone),
            CollisionLayers::new(
                LayerMask(lunco_core::TRIGGER_COLLISION_LAYER),
                LayerMask::ALL,
            ),
        ));
        if let Err(error) = add_collider_from_usd(commands, entity, reader, sdf_path) {
            log_malformed_collider_transform(sdf_path, &error);
            commands.entity(entity).try_insert(UsdAvianProcessed);
            return;
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }

    if has_rigid_body_api {
        // FIRST, before the `Collider` and `RigidBody` below, and that order is
        // load-bearing. `Commands` apply in insertion order and observers fire at
        // apply time, so avian's `On<Add, RigidBody>` mass observer (avian3d
        // `dynamics/rigid_body/mass_properties/mod.rs:284-289`) runs the instant
        // `RigidBody` lands. The overrides and their `NoAuto*` markers must already
        // be on the entity by then, or that observer derives
        // `ComputedAngularInertia` from collider geometry at `ColliderDensity` 1.0
        // and the authored values never take effect. Authoring the overrides first
        // means the observer's very first pass sees `NoAuto*` and honours them.
        let simulated =
            match read_authored_bool_or_default(reader, sdf_path, ptok::A_RIGID_BODY_ENABLED, true)
            {
                Ok(value) => value,
                Err(()) => {
                    error!(
                        "[usd-avian] {sdf_path} has malformed {} — refusing rigid-body projection",
                        ptok::A_RIGID_BODY_ENABLED
                    );
                    commands.entity(entity).try_insert(UsdAvianProcessed);
                    return;
                }
            };
        let kinematic =
            match read_authored_bool_or_default(reader, sdf_path, ptok::A_KINEMATIC_ENABLED, false)
            {
                Ok(value) => value,
                Err(()) => {
                    error!(
                        "[usd-avian] {sdf_path} has malformed {} — refusing rigid-body projection",
                        ptok::A_KINEMATIC_ENABLED
                    );
                    commands.entity(entity).try_insert(UsdAvianProcessed);
                    return;
                }
            };
        if apply_rigid_body_mass_props(commands, entity, reader, sdf_path).is_err() {
            error!(
                "[usd-avian] {sdf_path} has malformed rigid-body mass properties — refusing projection"
            );
            commands.entity(entity).try_insert(UsdAvianProcessed);
            return;
        }

        // ── COMPOUND BODY ROOT ── children colliders → compound, else self.
        let compound_shapes = match collect_child_colliders_from_usd(reader, sdf_path) {
            Ok(shapes) => shapes,
            Err(error) => {
                error!(
                    "[usd-avian] {sdf_path} has malformed descendant transform; refusing compound body: {error}"
                );
                commands.entity(entity).try_insert(UsdAvianProcessed);
                return;
            }
        };
        if !compound_shapes.is_empty() {
            commands
                .entity(entity)
                .try_insert(Collider::compound(compound_shapes));
        } else {
            if let Err(error) = add_collider_from_usd(commands, entity, reader, sdf_path) {
                log_malformed_collider_transform(sdf_path, &error);
                commands.entity(entity).try_insert(UsdAvianProcessed);
                return;
            }
        }
        apply_collision_groups(commands, entity, groups, sdf_path);

        // The schema's own `physics:rigidBodyEnabled` (default true) says whether
        // this body is simulated; a disabled body is unmoving collision geometry.
        // A `Dynamic`-declared body spawns `Kinematic` + `ShouldBeDynamic` and
        // settles to `Dynamic` once joints resolve (no 1-frame separation launch).
        let (body, mobility) = if !simulated {
            (RigidBody::Static, lunco_core::Mobility::Static)
        } else if kinematic {
            (RigidBody::Kinematic, lunco_core::Mobility::Kinematic)
        } else {
            commands
                .entity(entity)
                .try_insert((ShouldBeDynamic, lunco_core::PhysicsStatePending));
            (RigidBody::Kinematic, lunco_core::Mobility::Dynamic)
        };
        commands
            .entity(entity)
            .try_insert((body, mobility, lunco_core::SelectableRoot));
        if !simulated || kinematic {
            commands
                .entity(entity)
                .try_insert(lunco_core::PhysicsStateReady);
        }

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
        // standalone as one at `/Ground`, and must collide the same way.
        if !has_rigid_body_ancestor(reader, sdf_path) {
            if apply_physics_material(commands, entity, reader, sdf_path).is_err() {
                error!(
                    "[usd-avian] {sdf_path} has malformed physics-material values — refusing static collider projection"
                );
                commands.entity(entity).try_insert(UsdAvianProcessed);
                return;
            }
            commands
                .entity(entity)
                .try_insert((RigidBody::Static, lunco_core::Mobility::Static));
            if let Err(error) = add_collider_from_usd(commands, entity, reader, sdf_path) {
                log_malformed_collider_transform(sdf_path, &error);
                commands.entity(entity).try_insert(UsdAvianProcessed);
                return;
            }
            apply_collision_groups(commands, entity, groups, sdf_path);
        }
        commands.entity(entity).try_insert(UsdAvianProcessed);
    } else {
        // Neither a body nor a collider: no physics components, only the marker.
        commands.entity(entity).try_insert(UsdAvianProcessed);
    }
}

/// Put a collider on the layers its `PhysicsCollisionGroup` membership implies.
///
/// A no-op when the prim is in no group, and that matters: writing "collides with
/// everything" here would erase the explicit layers a trigger zone sets for
/// itself, and would make introducing one group elsewhere on the stage a silent
/// change to every other collider.
fn apply_collision_groups(
    commands: &mut Commands,
    entity: Entity,
    groups: &CollisionGroupTable,
    sdf_path: &SdfPath,
) {
    if let Some(layers) = groups.layers_for(&sdf_path.to_string()) {
        commands.entity(entity).try_insert(layers);
    }
}

/// The composed local-to-world [`Transform`] of `path`: folds the LOCAL transforms
/// (translate + rotate + **scale**) of every prim available in the read surface down
/// to it, so an ancestor's scale is baked into a descendant's world position — exactly
/// how the renderer places it. A prepared reference reader ends at its asset root;
/// scene ancestors are outside that read surface and contribute no local transform.
/// The walk skips absent outer ancestors until it reaches the first prim in the
/// read surface, then requires the remainder of the path to be contiguous. This
/// preserves the reference-instance boundary without silently bridging a missing
/// prim inside the composed asset.
/// An omitted xform stack composes as USD identity; malformed authored data is returned
/// as an error.
pub fn world_transform(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> Result<Transform, TransformReadError> {
    if !reader.has_prim(path) {
        return Err(TransformReadError {
            prim: path.to_string(),
        });
    }
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
    let mut in_read_surface = false;
    for p in chain.iter().rev() {
        // A prepared reference-instance reader deliberately contains the
        // composed asset subtree, not the owning scene's ancestors. Skip those
        // absent outer ancestors; once the asset root is found, a missing
        // interior prim stops composition at that boundary rather than inventing
        // a transform across the gap. An authored prim that is present still
        // goes through the strict transform reader below, so malformed USD
        // remains an error.
        if !reader.has_prim(p) {
            if in_read_surface {
                break;
            }
            continue;
        }
        in_read_surface = true;
        if let Some(local) = reader.local_transform_at(p, 0.0)? {
            acc = acc.mul_transform(local);
        }
    }
    Ok(acc)
}

/// Rotate a vector authored in a prim's local frame into the composed world
/// frame. USD physics velocity attributes are local-frame vectors, while
/// Avian's runtime velocity components are world-frame. Keep that rotation in
/// one helper so linear and angular initial state share the same convention.
fn local_vector_to_world(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    local: DVec3,
) -> Result<DVec3, TransformReadError> {
    Ok(world_transform(reader, path)?.rotation.as_dquat() * local)
}

/// Compose a prim's transform in the local frame of an authored body.
///
/// Physical mount points use the body origin and rotation, never the render
/// world's `GlobalTransform`. The relative transform is derived from the composed
/// USD read surface so nested component references and intermediate Xforms remain
/// valid without repeating the mount position in another description.
pub fn transform_in_body_frame(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    body_path: &SdfPath,
    prim_path: &SdfPath,
) -> Option<Transform> {
    let body = world_transform(reader, body_path).ok()?;
    let prim = world_transform(reader, prim_path).ok()?;
    let inv = body.rotation.inverse();
    Some(Transform {
        translation: inv * (prim.translation - body.translation),
        rotation: (inv * prim.rotation).normalize(),
        // Scale is not part of an Avian body frame. The position above already
        // contains authored ancestor scale, while force directions are vectors.
        scale: Vec3::ONE,
    })
}

/// Derive a joint's local anchors from the composed transform hierarchy, for the
/// **body1-origin** convention every rover joint uses: the joint sits at `body1`'s
/// origin, so its anchor on `body1` is the origin (`localPos1 = 0`) and its anchor on
/// `body0` is `body1`'s origin expressed in `body0`'s rotation frame. Returns
/// `(local_pos0, local_pos1)`.
///
/// This lets an asset author each part's placement ONCE — as the prim's
/// `xformOp:translate` — instead of typing it again as the joint's `physics:localPos0`.
/// `read_joint_spec` calls it only when the anchor is UNAUTHORED, so authored
/// joints are untouched (no regression) and hand-tuned anchors always win.
///
/// Uses WORLD poses, not an ancestor walk, so it is correct for **sibling** joints
/// (a rocker ↔ bogie hinge where neither body contains the other) and for **scaled**
/// hierarchies: `localPos0 = rot(world(b0))⁻¹ · (pos(world(b1)) − pos(world(b0)))`.
/// Ancestor scales are baked into the world positions; the anchor is expressed in
/// body0's rotation frame (avian applies a body's rotation — not its scale — to a
/// local anchor). Relative, hence invariant under the reference/path-translation that
/// drops a shared component onto each rover root.
fn derive_joint_anchor(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    body0: &str,
    body1: &str,
) -> Option<(DVec3, DVec3)> {
    let p0 = SdfPath::new(body0).ok()?;
    let p1 = SdfPath::new(body1).ok()?;
    let w0 = world_transform(reader, &p0).ok()?;
    let w1 = world_transform(reader, &p1).ok()?;
    let rel = w0.rotation.inverse() * (w1.translation - w0.translation);
    Some((
        DVec3::new(rel.x as f64, rel.y as f64, rel.z as f64),
        DVec3::ZERO,
    ))
}

/// Whether the standard wheel simulation owns the wheel endpoint of this joint.
///
/// Wheel revolute joints are built together with their wheel body by
/// `lunco-usd-sim`; the generic USD joint projector must not claim them. This
/// is resolved from the authored body relationship and applied wheel schema,
/// never from a prim name or a joint-name convention.
fn joint_targets_simulated_wheel(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> bool {
    let targets = reader.rel_targets(path, "physics:body1");
    if targets.len() != 1 {
        return false;
    }
    nearest_body_path(reader, &targets[0])
        .is_some_and(|body| reader.has_api_schema(&body, "PhysxVehicleWheelAPI"))
}

/// Read the STANDARD UsdPhysics joint at `path` through the shared composed
/// reader into the deferred [`PendingUsdJoint`]. The reader is either the
/// worker-produced initial projection plan or the live reader for an authored
/// generation; the joint contract is identical in both cases.
///
/// This reads the USD standard concrete joint type, shared body/frame
/// relationships, `UsdPhysicsDriveAPI`, and per-DOF `UsdPhysicsLimitAPI` for
/// the generic-D6 reduction. Returns `None` when
/// `path` is not a UsdPhysics joint, is missing a body ref, or targets a wheel
/// (owned by `lunco-usd-sim`). Revolute limits are converted degrees→radians
/// (the `PendingUsdJoint` contract); prismatic/distance stay in scene units.
fn read_joint_spec(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
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
pub(crate) fn read_joint_spec_for_lint(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> Option<PendingUsdJoint> {
    read_joint_spec_with_policy(reader, path, false)
}

fn read_joint_spec_with_policy(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    skip_lint_only: bool,
) -> Option<PendingUsdJoint> {
    let view = reader;
    // `lintOnly` is a narrow authoring contract for deliberately malformed
    // fixtures. It is checked only by the runtime reader; the linter calls the
    // companion entry point above so it still sees and evaluates the fixture.
    if skip_lint_only && view.boolean(path, "lunco:lintOnly") == Some(true) {
        return None;
    }
    // `physics:jointEnabled` (schema default true) is the spec's own way to say a
    // joint is not simulated. It matters now that COMPONENTS own their mount
    // joints: a host that wants the mechanism inert — `ground_station.usda` parks
    // its dish and disables both link bodies, because a station with no target to
    // track is better still than swinging — needs a way to say so without editing
    // the component. `over "YawJoint" { uniform bool physics:jointEnabled = false }`
    // is that way, and it is stock UsdPhysics rather than anything invented here.
    let joint_enabled = match view.boolean(path, ptok::A_JOINT_ENABLED) {
        Some(value) => value,
        None if !view.has_authored_attribute(path, ptok::A_JOINT_ENABLED) => true,
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
    let conv = lunco_usd_bevy::stage_convention(view).ok()?;
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
        if !(linear.is_finite() && linear >= 0.0 && angular.is_finite() && angular >= 0.0) {
            return None;
        }
        Some(JointDamping { linear, angular })
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
            ptok::AXIS_X => DVec3::X,
            ptok::AXIS_Y => DVec3::Y,
            ptok::AXIS_Z => DVec3::Z,
            _ => return None,
        }))
    };
    let read_axis = || -> Option<DVec3> {
        let axis = match view.text(path, ptok::A_AXIS) {
            Some(axis) if matches!(axis.as_str(), ptok::AXIS_X | ptok::AXIS_Y | ptok::AXIS_Z) => {
                axis
            }
            Some(_) => return None,
            None if !view.has_authored_attribute(path, ptok::A_AXIS) => ptok::AXIS_X.to_string(),
            None => return None,
        };
        axis_of(&axis)
    };
    // Shared JointBase reads (both bodies + local anchors). A rel left EMPTY is
    // the spec's way to anchor that side to the WORLD frame — carried through as
    // an empty body path, which the build arm realises as a static anchor body.
    // `None` when neither body is authored, or a NAMED target fails to resolve.
    // A missing anchor is DERIVED from the transform hierarchy (see
    // [`derive_joint_anchor`]) so an asset need not type the wheel's position twice
    // — once as its `xformOp:translate` and again as the joint's `localPos0`. An
    // authored anchor always wins.
    //
    // An AUTHORED anchor is a point in the stage's frame and units, so it converts
    // here. A DERIVED one must not: `derive_joint_anchor` builds it from
    // `world_transform` → `local_transform_at`, which already converted. Applying
    // the convention to both would double-convert the derived path.
    let base = || -> Option<JointBaseRead> {
        let conv = lunco_usd_bevy::stage_convention(reader).ok()?;
        // An endpoint that names a prim which is not itself a body resolves to
        // the body that prim is rigidly part of — see [`nearest_body_path`].
        // This is what lets a mounted mechanism name its own root instead of its
        // host's chassis. An exact hit is the normal case and costs one lookup.
        let resolve = |target: &SdfPath| -> Option<String> {
            Some(nearest_body_path(reader, target)?.to_string())
        };
        let targets0 = reader.rel_targets(path, ptok::A_BODY0);
        let targets1 = reader.rel_targets(path, ptok::A_BODY1);
        if targets0.len() > 1 || targets1.len() > 1 {
            return None;
        }
        let target0 = targets0.first();
        let target1 = targets1.first();
        let (b0, b1) = match (target0, target1) {
            (Some(t0), Some(t1)) => (resolve(&t0)?, resolve(&t1)?),
            (Some(t0), None) => (resolve(&t0)?, String::new()),
            (None, Some(t1)) => (String::new(), resolve(&t1)?),
            (None, None) => return None,
        };
        let lp0_auth = read_authored_vec3(reader, path, ptok::A_LOCAL_POS_0)
            .ok()?
            .map(|value| conv.point_d(value));
        let lp1_auth = read_authored_vec3(reader, path, ptok::A_LOCAL_POS_1)
            .ok()?
            .map(|value| conv.point_d(value));
        let (lp0, lp1) = if let (Some(lp0), Some(lp1)) = (lp0_auth, lp1_auth) {
            (lp0, lp1)
        } else {
            // A world side has no prim to derive from; its unauthored anchor is
            // the world origin.
            let derived = (!b0.is_empty() && !b1.is_empty())
                .then(|| derive_joint_anchor(reader, &b0, &b1))
                .flatten()?;
            (lp0_auth.unwrap_or(derived.0), lp1_auth.unwrap_or(derived.1))
        };
        let local_rot0 = read_authored_quat(reader, path, ptok::A_LOCAL_ROT_0)
            .ok()?
            .map(|value| conv.rotation_d(value))
            .unwrap_or(DQuat::IDENTITY);
        let local_rot1 = read_authored_quat(reader, path, ptok::A_LOCAL_ROT_1)
            .ok()?
            .map(|value| conv.rotation_d(value))
            .unwrap_or(DQuat::IDENTITY);
        Some(JointBaseRead {
            body0: b0,
            body1: b1,
            local_pos0: lp0,
            local_pos1: lp1,
            local_rot0,
            local_rot1,
        })
    };
    let read_generalized_inertia = |body_path: &str,
                                    axis_in_body: DVec3,
                                    joint_anchor_in_body: DVec3|
     -> Result<Option<f64>, ()> {
        if body_path.is_empty() {
            // A world anchor has infinite generalized inertia; its inverse
            // contribution is zero. The other body supplies the coordinate's
            // finite inertia.
            return Ok(None);
        }
        let body = SdfPath::new(body_path).map_err(|_| ())?;
        let Some(diagonal) = read_authored_vec3(view, &body, ptok::A_DIAGONAL_INERTIA)? else {
            return Ok(None);
        };
        if diagonal == DVec3::ZERO {
            // MassAPI's zero tensor is its unauthored sentinel.
            return Ok(None);
        }
        if !diagonal.is_finite() || diagonal.x <= 0.0 || diagonal.y <= 0.0 || diagonal.z <= 0.0 {
            return Err(());
        }
        let principal_axes = read_authored_quat(view, &body, ptok::A_PRINCIPAL_AXES)?
            .map(|q| conv.rotation_d(q))
            .unwrap_or(DQuat::IDENTITY);
        let mpu = conv.length(1.0);
        let principal = conv.dir_d(diagonal).abs() * (mpu * mpu);
        let axis = axis_in_body.normalize_or_zero();
        if !axis.is_finite() || axis.length_squared() <= f64::EPSILON {
            return Err(());
        }
        let principal_axis = principal_axes.inverse() * axis;
        let rotational = principal_axis.x * principal_axis.x * principal.x
            + principal_axis.y * principal_axis.y * principal.y
            + principal_axis.z * principal_axis.z * principal.z;
        if !rotational.is_finite() || rotational <= f64::EPSILON {
            return Err(());
        }
        // USD's diagonal inertia is about the body's center of mass. A joint
        // hinge is generally offset from that point, so the scalar inertia of
        // the generalized coordinate also contains m * r_perp^2. This is the
        // standard rigid-body parallel-axis term and is essential for steering
        // knuckles mounted below a rover chassis.
        let mass = match read_authored_real(view, &body, ptok::A_MASS)? {
            // A missing/zero mass is valid USD authoring: Avian derives it from
            // the collider tree. Do not treat that as a zero-mass body while
            // lowering the drive; leave the coordinate unresolved so the live
            // Avian properties can supply the correct value.
            None | Some(0.0) => return Ok(None),
            Some(value) if value.is_finite() && value > 0.0 => value,
            Some(_) => return Err(()),
        };
        let center_of_mass = read_authored_vec3(view, &body, ptok::A_CENTER_OF_MASS)?
            .map(|value| conv.point_d(value))
            .unwrap_or(DVec3::ZERO);
        let offset = joint_anchor_in_body - center_of_mass;
        if !center_of_mass.is_finite() || !offset.is_finite() {
            return Err(());
        }
        let perpendicular_offset_squared =
            (offset.length_squared() - offset.dot(axis) * offset.dot(axis)).max(0.0);
        let scalar = rotational + mass * perpendicular_offset_squared;
        if !scalar.is_finite() || scalar <= f64::EPSILON {
            return Err(());
        }
        Ok(Some(scalar))
    };
    let read_drive = |ns: &str,
                      body0: &str,
                      body1: &str,
                      axis: DVec3,
                      local_pos0: DVec3,
                      local_pos1: DVec3,
                      local_rot0: DQuat,
                      local_rot1: DQuat|
     -> Result<Option<JointDrive>, ()> {
        let api_name = format!("PhysicsDriveAPI:{ns}");
        if !view.has_api_schema(path, &api_name) {
            return Ok(None);
        }
        // Drive quantities convert by their SPEC units, per instance. An angular
        // drive authors DEGREES (`targetPosition` deg, `targetVelocity` deg/s,
        // stiffness/damping per degree) and its torques carry distance² — so
        // targets go deg→rad, gains ×(180/π), and every torque ×metersPerUnit².
        // A linear drive authors stage units: targets and `maxForce` (mass ·
        // distance / s²) scale by metersPerUnit; its stiffness (mass/s²) and
        // damping (mass/s) are distance-free and pass through.
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
                conv.length(conv.length(v.to_degrees()))
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
        let property = |sub: &str| format!("drive:{ns}:physics:{sub}");
        let target_position_name = property(ptok::DRIVE_SUB_TARGET_POSITION);
        let target_velocity_name = property(ptok::DRIVE_SUB_TARGET_VELOCITY);
        let max_force_name = property(ptok::DRIVE_SUB_MAX_FORCE);
        let stiffness_name = property(ptok::DRIVE_SUB_STIFFNESS);
        let damping_name = property(ptok::DRIVE_SUB_DAMPING);
        let tp = read_value(&target_position_name)?.map(target);
        let tv = read_value(&target_velocity_name)?.map(target);
        let mf = read_value(&max_force_name)?.map(force);
        let k = read_value(&stiffness_name)?.map(gain);
        let c = read_value(&damping_name)?.map(gain);
        let type_name = property(ptok::DRIVE_SUB_TYPE);
        let ty = match view.attr_value(path, &type_name) {
            Some(value) => Some(value.get::<DriveType>().ok_or(())?),
            None if !view.has_authored_attribute(path, &type_name) => None,
            None => return Err(()),
        };
        // The force-spring → SpringDamper conversion uses the generalized inertia
        // of the coordinate. When USD authors complete mass properties, the fact
        // is recorded here. When it omits them (the schema-valid computed path),
        // the runtime resolver obtains the effective mass/moment from Avian after
        // collider and density processing.
        let generalized_inertia = if ns == ptok::DOF_LINEAR {
            // A world endpoint has infinite mass. A non-world endpoint with no
            // authored mass is still valid USD and will be resolved from
            // Avian's computed body mass during joint construction.
            if body1.is_empty() {
                None
            } else {
                let b1 = SdfPath::new(body1).map_err(|_| ())?;
                match read_authored_real(view, &b1, ptok::A_MASS)? {
                    None | Some(0.0) => None,
                    Some(value) if value.is_finite() && value > 0.0 => Some(value),
                    Some(_) => return Err(()),
                }
            }
        } else {
            let i0 = read_generalized_inertia(body0, local_rot0 * axis, local_pos0)?;
            let i1 = read_generalized_inertia(body1, local_rot1 * axis, local_pos1)?;
            // `None` means either a world endpoint or an endpoint whose
            // properties are computed by Avian. Only combine authored values
            // when every non-world endpoint supplied a complete tensor.
            let authored0 = body0.is_empty() || i0.is_some();
            let authored1 = body1.is_empty() || i1.is_some();
            if !authored0 || !authored1 {
                None
            } else {
                let inverse =
                    i0.map(|i| 1.0 / i).unwrap_or(0.0) + i1.map(|i| 1.0 / i).unwrap_or(0.0);
                (inverse > f64::EPSILON && inverse.is_finite()).then_some(1.0 / inverse)
            }
        };
        Ok(
            (tp.is_some() || tv.is_some() || mf.is_some() || k.is_some() || c.is_some()).then_some(
                JointDrive {
                    target_position: tp,
                    target_velocity: tv,
                    max_force: mf,
                    stiffness: k,
                    damping: c,
                    drive_type: ty,
                    generalized_inertia,
                },
            ),
        )
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
        ptok::T_PHYSICS_REVOLUTE_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            let lo = read_limit(ptok::A_LOWER_LIMIT, f64::NEG_INFINITY)?.to_radians();
            let hi = read_limit(ptok::A_UPPER_LIMIT, f64::INFINITY)?.to_radians();
            let drive = read_drive(
                ptok::DOF_ANGULAR,
                &b.body0,
                &b.body1,
                axis,
                b.local_pos0,
                b.local_pos1,
                b.local_rot0,
                b.local_rot1,
            )
            .ok()?;
            pending_from(b, axis, lo, hi, ptok::T_PHYSICS_REVOLUTE_JOINT, None, drive)
        }
        ptok::T_PHYSICS_PRISMATIC_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            // Linear limits are authored in scene units, like the anchors.
            let lo = conv.length(read_limit(ptok::A_LOWER_LIMIT, f64::NEG_INFINITY)?);
            let hi = conv.length(read_limit(ptok::A_UPPER_LIMIT, f64::INFINITY)?);
            let drive = read_drive(
                ptok::DOF_LINEAR,
                &b.body0,
                &b.body1,
                axis,
                b.local_pos0,
                b.local_pos1,
                b.local_rot0,
                b.local_rot1,
            )
            .ok()?;
            pending_from(
                b,
                axis,
                lo,
                hi,
                ptok::T_PHYSICS_PRISMATIC_JOINT,
                None,
                drive,
            )
        }
        ptok::T_PHYSICS_SPHERICAL_JOINT => {
            let b = base()?;
            let axis = read_axis()?;
            // `physics:coneAngle{0,1}Limit` is in degrees, while Avian's
            // `AngleLimit` is in radians.
            let cone0 = read_limit(ptok::A_CONE_ANGLE_0_LIMIT, -1.0)?;
            let cone1 = read_limit(ptok::A_CONE_ANGLE_1_LIMIT, -1.0)?;
            let swing = (cone0 >= 0.0 || cone1 >= 0.0)
                .then_some((cone0.max(0.0).to_radians(), cone1.max(0.0).to_radians()));
            pending_from(
                b,
                axis,
                f64::NEG_INFINITY,
                f64::INFINITY,
                ptok::T_PHYSICS_SPHERICAL_JOINT,
                swing,
                None,
            )
        }
        ptok::T_PHYSICS_FIXED_JOINT => {
            let b = base()?;
            pending_from(
                b,
                DVec3::Y,
                f64::NEG_INFINITY,
                f64::INFINITY,
                ptok::T_PHYSICS_FIXED_JOINT,
                None,
                None,
            )
        }
        ptok::T_PHYSICS_DISTANCE_JOINT => {
            let b = base()?;
            // Distances are scene units; a negative authored value is the
            // schema's "limit disabled" sentinel.
            let lo = conv.length(read_limit(ptok::A_MIN_DISTANCE, -1.0)?);
            let hi = conv.length(read_limit(ptok::A_MAX_DISTANCE, -1.0)?);
            pending_from(
                b,
                DVec3::Y,
                lo,
                hi,
                ptok::T_PHYSICS_DISTANCE_JOINT,
                None,
                None,
            )
        }
        ptok::T_PHYSICS_JOINT => {
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

    // Wheel-targeted joints are owned by `lunco-usd-sim` (built alongside the
    // wheel body); skip them here to avoid double-up/race.
    if joint_targets_simulated_wheel(view, path) {
        return None;
    }
    Some(spec)
}

/// Reduce a generic `UsdPhysicsJoint` (D6) to the Avian primitive matching its
/// free degrees of freedom by reading each per-DOF `UsdPhysicsLimitAPI`
/// (`limit:{transX..rotZ}`). A DOF is locked when `low > high` and free when
/// the limit schema is absent or its bounds are unauthored.
fn reduce_generic_joint(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
) -> Option<(&'static str, DVec3, f64, f64, bool)> {
    const DOFS: [(&str, DVec3, bool); 6] = [
        (ptok::DOF_TRANS_X, DVec3::X, false),
        (ptok::DOF_TRANS_Y, DVec3::Y, false),
        (ptok::DOF_TRANS_Z, DVec3::Z, false),
        (ptok::DOF_ROT_X, DVec3::X, true),
        (ptok::DOF_ROT_Y, DVec3::Y, true),
        (ptok::DOF_ROT_Z, DVec3::Z, true),
    ];
    let mut free_trans: Vec<(DVec3, f64, f64)> = Vec::new();
    let mut free_rot: Vec<(DVec3, f64, f64)> = Vec::new();
    for (inst, axis, is_rot) in DOFS {
        let api_name = format!("{}:{inst}", ptok::API_LIMIT);
        let property = |bound: &str| format!("limit:{inst}:physics:{bound}");
        let read_bound = |name: &str, default: f64| match reader.real(path, name) {
            Some(value) if !value.is_nan() => Some(value),
            None if !reader.has_authored_attribute(path, name) => Some(default),
            _ => None,
        };
        let low_name = property(ptok::LIMIT_SUB_LOW);
        let high_name = property(ptok::LIMIT_SUB_HIGH);
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

/// Observer that fires when a USD prim entity is added.
///
/// Detects physics joints (PhysicsRevoluteJoint, PhysicsPrismaticJoint, …) and
/// stamps the deferred [`PendingUsdJoint`] carrier from the standard composed
/// UsdPhysics joint attributes through the shared reader boundary.
fn on_add_usd_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<(&UsdPrimPath, Option<&UsdInstanceProjection>)>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy::CanonicalStages>,
    mut commands: Commands,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    let entity = trigger.entity;
    let Ok((prim_path, instance_projection)) = query.get(entity) else {
        return;
    };
    // Joint authoring is shared with the visual projection, but Editor
    // previews are render-only.  This observer runs independently of the
    // `UsdVisualSynced` Avian guard, so it must enforce the same ownership
    // boundary or a preview hinge becomes a live pending joint forever.
    if is_preview_only(entity, &q_child_of, &q_preview_only) {
        return;
    }
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
        return;
    };

    let id = prim_path.stage_handle.id();
    let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
        return;
    };
    let (reader, _generation) = canonical.reader_for_entity(id, stage_asset, instance_projection);
    if reader
        .real_f32(&sdf_path, "physxVehicleWheel:radius")
        .is_some()
    {
        return;
    }
    let wheel_owned = joint_targets_simulated_wheel(&reader, &sdf_path);
    if wheel_owned {
        return;
    }
    // Authoring/lint fixtures remain in the composed stage so the linter can
    // inspect their faults, but they are not runtime constraints.  Apply this
    // admission policy before the invalid-joint fault path: a deliberately
    // malformed guide joint must not poison the live physics hold gate.
    if reader.boolean(&sdf_path, "lunco:lintOnly") == Some(true) {
        return;
    }
    project_pending_joint(
        &reader,
        entity,
        &sdf_path,
        &mut commands,
        faults.as_deref_mut(),
        holds.as_deref_mut(),
    );

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

/// Physics ticks a pending joint may scan the body query at full rate before its
/// unresolved body path is reported (a typo'd rel never spawns, and a silent
/// forever-scan is exactly the failure mode this project pays most for).
const JOINT_RESOLVE_WARN_TICKS: u32 = 600;

/// Retry cadence for a pending joint after its warning budget.
const JOINT_RESOLVE_RETRY_INTERVAL: u32 = 60;

/// Hard deadline for a joint whose authored body relationship never resolves.
/// Once reached the marker is removed and the scene receives a terminal fault;
/// readiness must not remain open forever on a typo'd relationship.
const JOINT_RESOLVE_MAX_TICKS: u32 = 3_600;

/// Return body1's velocity after seating a joint without asking the solver to
/// remove an authored constraint violation on its first step.
///
/// The admitted velocity must obey the same degrees of freedom as the joint:
/// fixed has none, prismatic preserves axial translation, revolute preserves
/// angular rate about its hinge, and spherical preserves all relative angular
/// rate. Every one of those joints still locks the two anchor points together.
/// A child without authored velocity inherits the parent's rigid motion;
/// treating it as stationary is an impulse request at the joint anchor.
fn seated_body1_velocity(
    body0_position: DVec3,
    body1_position: DVec3,
    anchor_world: DVec3,
    body0_linear: DVec3,
    body0_angular: DVec3,
    body1_linear: DVec3,
    body1_angular: DVec3,
    free_linear_axis_world: Option<DVec3>,
    free_angular_axis_world: Option<DVec3>,
    all_angular_free: bool,
    preserve_authored_free_rates: bool,
) -> (DVec3, DVec3) {
    let body0_anchor_offset = anchor_world - body0_position;
    let body1_anchor_offset = anchor_world - body1_position;
    let body0_anchor_velocity = body0_linear + body0_angular.cross(body0_anchor_offset);
    let free_linear_rate = free_linear_axis_world
        .filter(|_| preserve_authored_free_rates)
        .map(|axis| {
            let body1_anchor_velocity = body1_linear + body1_angular.cross(body1_anchor_offset);
            (body1_anchor_velocity - body0_anchor_velocity).dot(axis)
        })
        .unwrap_or(0.0);
    let target_angular = if !preserve_authored_free_rates {
        body0_angular
    } else if all_angular_free {
        body1_angular
    } else if let Some(axis) = free_angular_axis_world {
        body0_angular + axis * (body1_angular - body0_angular).dot(axis)
    } else {
        body0_angular
    };
    let target_anchor_velocity = free_linear_axis_world
        .map(|axis| body0_anchor_velocity + axis * free_linear_rate)
        .unwrap_or(body0_anchor_velocity);
    let target_linear = target_anchor_velocity - target_angular.cross(body1_anchor_offset);
    (target_linear, target_angular)
}

/// The authored frame and free degrees of freedom used to seat a pending
/// constraint before Avian's first solve.
///
/// USD joints and synthesized wheel joints use the same admission boundary.
/// Keeping the frame here means a synthesized constraint cannot bypass the
/// position/orientation/velocity projection that authored joints receive.
#[derive(Clone, Copy, Debug)]
enum JointSeatKind {
    Fixed,
    Prismatic,
    Revolute,
    Spherical,
    Distance,
}

#[derive(Clone, Copy, Debug)]
struct JointSeat {
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
    axis: DVec3,
    kind: JointSeatKind,
}

impl JointSeat {
    fn usd(pending: &PendingUsdJoint) -> Option<Self> {
        let kind = match pending.joint_type.as_str() {
            "PhysicsFixedJoint" => JointSeatKind::Fixed,
            "PhysicsPrismaticJoint" => JointSeatKind::Prismatic,
            "PhysicsRevoluteJoint" => JointSeatKind::Revolute,
            "PhysicsSphericalJoint" => JointSeatKind::Spherical,
            "PhysicsDistanceJoint" => JointSeatKind::Distance,
            _ => return None,
        };
        Some(Self {
            local_pos0: pending.local_pos0,
            local_pos1: pending.local_pos1,
            local_rot0: pending.local_rot0,
            local_rot1: pending.local_rot1,
            axis: pending.axis,
            kind,
        })
    }
}

/// Seat a joint's body1 against body0's authored frame and project its initial
/// velocity onto the joint's actual free degrees of freedom.
///
/// This is deliberately called at the common pending-joint admission boundary,
/// after both Avian body states exist and before the next fixed solve. It is the
/// one startup state transition for both authored USD joints and synthesized
/// wheel joints. A missing state is not guessed: the pending joint remains
/// admissible, and Avian's normal body initialization owns that endpoint.
fn seat_joint_bodies(
    label: &str,
    body0: Entity,
    body1: Entity,
    seat: JointSeat,
    q_pose: &mut Query<(&mut Position, &mut Rotation)>,
    q_vel: &mut Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    q_authored_velocity: &Query<&AuthoredInitialVelocity>,
    commands: &mut Commands,
) {
    let pose0 = q_pose
        .get(body0)
        .ok()
        .map(|(position, rotation)| (GridPos(position.0), rotation.0));
    let pose1 = q_pose
        .get(body1)
        .ok()
        .map(|(position, rotation)| (GridPos(position.0), rotation.0));
    let (Some((p0, r0)), Some((p1, r1))) = (pose0, pose1) else {
        return;
    };

    let locks_rotation = matches!(seat.kind, JointSeatKind::Fixed | JointSeatKind::Prismatic);
    let r1_target = r0 * seat.local_rot0 * seat.local_rot1.inverse();
    let angle = if locks_rotation {
        r1.angle_between(r1_target)
    } else {
        0.0
    };
    let r1_seated = if locks_rotation { r1_target } else { r1 };
    let anchor0_world = p0 + r0 * seat.local_pos0;
    let anchor1_world = p1 + r1_seated * seat.local_pos1;
    let delta = anchor0_world - anchor1_world;
    let p1_seated = p1 + delta;
    let seat_pos = delta.length() > JOINT_SEAT_EPS;
    let seat_rot = angle > JOINT_SEAT_ANGLE_EPS;

    if seat_pos || seat_rot {
        let worst = delta.length().max(angle);
        let detail = format!(
            "[usd-avian] joint {label} starts violated by {:.3} m / {:.3} rad — seating body1 {:?} onto the authored joint frame",
            delta.length(),
            angle,
            body1,
        );
        if worst > JOINT_SEAT_ERROR_THRESHOLD {
            error!("{detail}");
        } else {
            warn!("{detail}");
        }
        if let Ok((mut position, mut rotation)) = q_pose.get_mut(body1) {
            if seat_rot {
                rotation.0 = r1_target;
            }
            if seat_pos {
                position.0 += delta;
            }
        }
    }

    let seats_anchor_velocity = matches!(
        seat.kind,
        JointSeatKind::Fixed
            | JointSeatKind::Prismatic
            | JointSeatKind::Revolute
            | JointSeatKind::Spherical
    );
    if !seats_anchor_velocity {
        return;
    }

    let authored0 = q_authored_velocity.get(body0).ok().copied();
    let authored1 = q_authored_velocity.get(body1).ok().copied();
    let Some((lin0, ang0)) = q_vel.get(body0).ok().map(|(linear, angular)| {
        (
            authored0
                .and_then(|velocity| velocity.linear)
                .unwrap_or(linear.0),
            authored0
                .and_then(|velocity| velocity.angular)
                .unwrap_or(angular.0),
        )
    }) else {
        return;
    };
    let Some((lin1, ang1)) = q_vel.get(body1).ok().map(|(linear, angular)| {
        (
            authored1
                .and_then(|velocity| velocity.linear)
                .unwrap_or(linear.0),
            authored1
                .and_then(|velocity| velocity.angular)
                .unwrap_or(angular.0),
        )
    }) else {
        return;
    };

    let joint_axis_world = (r0 * seat.local_rot0 * seat.axis).normalize_or_zero();
    let free_linear_axis_world =
        matches!(seat.kind, JointSeatKind::Prismatic).then_some(joint_axis_world);
    let free_angular_axis_world =
        matches!(seat.kind, JointSeatKind::Revolute).then_some(joint_axis_world);
    let all_angular_free = matches!(seat.kind, JointSeatKind::Spherical);
    let (target_lin, target_ang) = seated_body1_velocity(
        p0.0,
        p1_seated.0,
        anchor0_world.0,
        lin0,
        ang0,
        lin1,
        ang1,
        free_linear_axis_world,
        free_angular_axis_world,
        all_angular_free,
        authored1.is_some_and(|velocity| velocity.linear.is_some() || velocity.angular.is_some()),
    );
    if (lin1 - target_lin).length() > JOINT_SEAT_EPS
        || (ang1 - target_ang).length() > JOINT_SEAT_ANGLE_EPS
    {
        if let Ok((mut linear, mut angular)) = q_vel.get_mut(body1) {
            linear.0 = target_lin;
            angular.0 = target_ang;
        }
        // The authored child velocity has now been projected through its joint
        // contract. Dynamic admission must not reapply the unconstrained value.
        commands
            .entity(body1)
            .try_remove::<AuthoredInitialVelocity>();
    }
}

#[cfg(test)]
mod joint_velocity_tests {
    use super::seated_body1_velocity;
    use bevy::math::DVec3;

    #[test]
    fn prismatic_child_inherits_parent_motion_but_keeps_slider_rate_free() {
        let parent_velocity = DVec3::new(0.6, -0.25, 0.3);
        let slider_axis = DVec3::new(0.34202014, -0.93969262, 0.0);
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(2.5, -4.0, 0.0),
            DVec3::new(2.5, 0.0, 0.0),
            parent_velocity,
            DVec3::ZERO,
            DVec3::ZERO,
            DVec3::ZERO,
            Some(slider_axis),
            None,
            false,
            false,
        );

        assert!((linear - parent_velocity).length() < 1.0e-6);
        assert_eq!(angular, DVec3::ZERO);
    }

    #[test]
    fn prismatic_child_preserves_only_an_authored_slider_rate() {
        let parent_velocity = DVec3::new(0.6, -0.25, 0.3);
        let slider_axis = DVec3::new(0.34202014, -0.93969262, 0.0);
        let child_velocity = parent_velocity + slider_axis * 1.75;
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(2.5, -4.0, 0.0),
            DVec3::new(2.5, 0.0, 0.0),
            parent_velocity,
            DVec3::ZERO,
            child_velocity,
            DVec3::ZERO,
            Some(slider_axis),
            None,
            false,
            true,
        );

        assert!((linear - child_velocity).length() < 1.0e-6);
        assert_eq!(angular, DVec3::ZERO);
    }

    #[test]
    fn spherical_child_inherits_rigid_motion_when_rates_are_unauthored() {
        let parent_linear = DVec3::new(0.8, -2.6, 1.5);
        let parent_angular = DVec3::new(0.1, -0.2, 0.3);
        let child_position = DVec3::new(2.5, -5.0, 0.0);
        let anchor = DVec3::new(2.5, -4.9, 0.0);
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            child_position,
            anchor,
            parent_linear,
            parent_angular,
            DVec3::ZERO,
            DVec3::ZERO,
            None,
            None,
            true,
            false,
        );

        let expected_anchor_velocity = parent_linear + parent_angular.cross(anchor);
        let actual_anchor_velocity = linear + angular.cross(anchor - child_position);
        assert!((actual_anchor_velocity - expected_anchor_velocity).length() < 1.0e-6);
        assert_eq!(angular, parent_angular);
    }

    #[test]
    fn revolute_child_preserves_only_authored_hinge_rate() {
        let hinge = DVec3::Y;
        let parent_angular = DVec3::new(0.1, -0.2, 0.3);
        let child_angular = parent_angular + hinge * 1.75 + DVec3::X * 4.0;
        let (linear, angular) = seated_body1_velocity(
            DVec3::ZERO,
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(0.5, 0.0, 0.0),
            DVec3::ZERO,
            parent_angular,
            DVec3::ZERO,
            child_angular,
            None,
            Some(hinge),
            false,
            true,
        );

        assert!(((angular - parent_angular).dot(hinge) - 1.75).abs() < 1.0e-6);
        assert!((angular.x - parent_angular.x).abs() < 1.0e-6);
        let body0_anchor_velocity = parent_angular.cross(DVec3::new(0.5, 0.0, 0.0));
        let body1_anchor_velocity = linear + angular.cross(DVec3::new(-0.5, 0.0, 0.0));
        assert!((body1_anchor_velocity - body0_anchor_velocity).length() < 1.0e-6);
    }
}

fn build_usd_physics_joints(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingUsdJoint, &UsdPrimPath)>,
    // Preparation may run while the world-readiness hold has paused Avian's
    // nested PhysicsSchedule. These filters establish that both USD endpoints
    // are live rigid bodies with a Position slot. They may still carry
    // `RigidBodyDisabled` while readiness freezes the authored subtree; that
    // marker means "do not integrate yet", not "the authored body does not
    // exist". They do NOT claim island admission. `attach_joint` parks the
    // constraint as `PendingJoint`, and `JointAttachPlugin` is the sole owner of
    // admitting that parked constraint after Avian creates both island nodes.
    //
    // `Position` is still only pose storage until `q_shadow` below confirms the
    // bridge has seeded it. Keeping those two facts separate prevents seating
    // against the required-component default at the origin.
    q_bodies: Query<(Entity, &UsdPrimPath), (With<RigidBody>, With<Position>)>,
    // Avian owns the composed mass properties. USD mass/inertia overrides and
    // collider/density-derived values both arrive here after its prepare pass;
    // the drive resolver below uses this query only when USD did not author a
    // generalized inertia explicitly.
    q_mass_properties: Query<(
        &RigidBody,
        Option<&ShouldBeDynamic>,
        Option<&ComputedMass>,
        Option<&ComputedAngularInertia>,
        Option<&ComputedCenterOfMass>,
    )>,
    // **Pose readiness gate**: has the physics-transform bridge written a real
    // world pose into `Position` yet? See `BridgeShadow::is_seeded`.
    q_shadow: Query<&big_space_bridge::BridgeShadow>,
    // Ground placement may have authored the final active-frame pose directly
    // while the bridge was intentionally excluded from that transaction. That
    // marker is the provenance for an already-valid Position in that case.
    q_pose_authoritative: Query<(), With<lunco_core::PhysicsPoseAuthoritative>>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    q_instance_projection: Query<&UsdInstanceProjection>,
    q_pose: Query<(&Position, &Rotation)>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut resolve_ticks: Local<EntityHashMap<u32>>,
) {
    resolve_ticks.retain(|e, _| q_pending.contains(*e));
    for (joint_entity, pending, joint_prim_path) in q_pending.iter() {
        // Joint preparation is intentionally allowed while world readiness
        // holds. The hold pauses integration, not topology construction:
        // `attach_joint` parks the native constraint and `JointAttachPlugin`
        // admits it after Avian creates the solver body-island nodes. Holding
        // this builder would deadlock readiness because the binding epoch waits
        // for the pending joint marker to clear.
        let ticks = resolve_ticks.get(&joint_entity).copied().unwrap_or(0);
        if ticks >= JOINT_RESOLVE_WARN_TICKS && ticks % JOINT_RESOLVE_RETRY_INTERVAL != 0 {
            resolve_ticks.insert(joint_entity, ticks.saturating_add(1));
            continue;
        }
        let joint_root = instance_key(
            joint_entity,
            &q_provenance,
            &q_gid,
            &q_instance_root,
            &q_instance_projection,
        );
        // Find body0 and body1 entities by matching USD paths and instance roots.
        // The paths were already resolved to real bodies at parse time (see
        // [`nearest_body_path`]), so this is an exact match by construction. An
        // EMPTY path is a world-anchored side (spec: an unauthored body rel) and
        // resolves to a fresh static anchor body below instead of a lookup.
        let world0 = pending.body0_path.is_empty();
        let world1 = pending.body1_path.is_empty();
        let body0_ent = q_bodies
            .iter()
            .find(|(e, path)| {
                path.path == pending.body0_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && instance_key(
                        *e,
                        &q_provenance,
                        &q_gid,
                        &q_instance_root,
                        &q_instance_projection,
                    ) == joint_root
            })
            .map(|(e, _)| e);
        let body1_ent = q_bodies
            .iter()
            .find(|(e, path)| {
                path.path == pending.body1_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && instance_key(
                        *e,
                        &q_provenance,
                        &q_gid,
                        &q_instance_root,
                        &q_instance_projection,
                    ) == joint_root
            })
            .map(|(e, _)| e);

        let missing0 = !world0 && body0_ent.is_none();
        let missing1 = !world1 && body1_ent.is_none();
        if missing0 || missing1 {
            let ticks = ticks.saturating_add(1);
            if ticks == JOINT_RESOLVE_WARN_TICKS {
                let missing = match (missing0, missing1) {
                    (true, true) => format!(
                        "bodies '{}' and '{}'",
                        pending.body0_path, pending.body1_path
                    ),
                    (true, _) => format!("body '{}'", pending.body0_path),
                    _ => format!("body '{}'", pending.body1_path),
                };
                let candidates: Vec<String> = q_bodies
                    .iter()
                    .filter(|(_, path)| {
                        path.path == pending.body0_path || path.path == pending.body1_path
                    })
                    .map(|(entity, path)| {
                        format!(
                            "entity={entity:?} path={} stage={:?} instance={:?}",
                            path.path,
                            path.stage_handle.id(),
                            instance_key(
                                entity,
                                &q_provenance,
                                &q_gid,
                                &q_instance_root,
                                &q_instance_projection,
                            ),
                        )
                    })
                    .collect();
                warn!(
                    "[usd-avian] joint {}: {missing} still unresolved after {} physics ticks \
                     — check the joint's body rel paths and stage/instance identity; \
                     candidates={candidates:?}; retrying every {} ticks.",
                    joint_prim_path.path, JOINT_RESOLVE_WARN_TICKS, JOINT_RESOLVE_RETRY_INTERVAL,
                );
            }
            if ticks >= JOINT_RESOLVE_MAX_TICKS {
                let detail = format!(
                    "body relationship did not resolve after {} physics ticks: body0='{}', \
                     body1='{}'",
                    JOINT_RESOLVE_MAX_TICKS, pending.body0_path, pending.body1_path
                );
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "usd-joint-unresolved",
                        Some(joint_entity),
                        joint_prim_path.path.clone(),
                        detail.clone(),
                    );
                }
                error!(
                    "[usd-avian] joint {} is terminally unresolved: {detail}",
                    joint_prim_path.path
                );
                commands
                    .entity(joint_entity)
                    .remove::<PendingUsdJoint>()
                    .remove::<lunco_physics::PhysicsJointPending>();
                resolve_ticks.remove(&joint_entity);
                continue;
            }
            resolve_ticks.insert(joint_entity, ticks);
            continue;
        }
        resolve_ticks.remove(&joint_entity);

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
        let seeded = |e: Entity| {
            q_pose_authoritative.contains(e)
                || q_shadow.get(e).map(|s| s.is_seeded()).unwrap_or(true)
        };
        if body0_ent.is_some_and(|e| !seeded(e)) || body1_ent.is_some_and(|e| !seeded(e)) {
            debug!(
                "[usd-avian] joint {} — body poses not seeded by the physics-transform \
                 bridge yet; deferring the joint rather than seating it against \
                 uninitialised positions.",
                joint_prim_path.path,
            );
            continue;
        }

        // Snapshot poses before the seating write below. They are also the
        // world-frame data needed for a live angular inertia calculation. A
        // world/static endpoint has no finite mass properties and contributes
        // zero inverse inertia to the effective coordinate.
        let drive_pose0 = body0_ent
            .and_then(|entity| q_pose.get(entity).ok().map(|(p, r)| (GridPos(p.0).0, r.0)));
        let drive_pose1 = body1_ent
            .and_then(|entity| q_pose.get(entity).ok().map(|(p, r)| (GridPos(p.0).0, r.0)));
        let resolved_drive_model = match pending.drive {
            None => None,
            Some(drive) => match resolve_joint_drive_motor_model(
                drive,
                pending,
                body0_ent,
                body1_ent,
                drive_pose0,
                drive_pose1,
                &q_mass_properties,
            ) {
                ResolvedJointDrive::Ready(model) => Some(model),
                ResolvedJointDrive::Waiting => {
                    // USD permits mass/inertia to be computed from attached
                    // colliders. Avian has not exposed that result yet; keep
                    // the authored joint pending and retry after the next
                    // mass-property update.
                    continue;
                }
                ResolvedJointDrive::Invalid(error) => {
                    let detail = format!(
                        "standard USD force drive could not be realized from authored or computed generalized inertia: {error:?}; ensure participating bodies have valid mass properties or attached colliders"
                    );
                    error!(
                        "USD physics joint {} rejected its drive: {detail}",
                        joint_prim_path.path
                    );
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "usd-physics-joint-drive-invalid",
                            Some(joint_entity),
                            joint_prim_path.path.clone(),
                            detail,
                        );
                    }
                    if let Some(holds) = holds.as_deref_mut() {
                        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                    }
                    commands
                        .entity(joint_entity)
                        .remove::<PendingUsdJoint>()
                        .remove::<lunco_physics::PhysicsJointPending>();
                    resolve_ticks.remove(&joint_entity);
                    continue;
                }
            },
        };

        // A world-anchored side becomes a static body at the canonical origin,
        // so the authored `localPos`/`localRot` — expressed in the world frame
        // when the rel is empty — apply as that anchor's local frame unchanged.
        // A static body is admitted by construction (see
        // [`admit_pending_joints`]). These entities are spawned only after the
        // drive resolver succeeds, so a deferred computed-property update never
        // leaks one anonymous anchor per retry.
        let b0 = body0_ent
            .unwrap_or_else(|| commands.spawn((RigidBody::Static, ScenePhysicsOwned)).id());
        let b1 = body1_ent
            .unwrap_or_else(|| commands.spawn((RigidBody::Static, ScenePhysicsOwned)).id());

        debug!(
            "Built USD joint {} -> {} <-> {}",
            pending.joint_type, pending.body0_path, pending.body1_path,
        );

        // The one seating contract is shared by authored USD joints and
        // synthesized wheel joints. It runs at the pending-joint admission
        // boundary, after the body states exist and before the next solver step.
        // Put the avian joint component ON the joint prim entity itself (it
        // already carries `UsdPrimPath` + the loader-assigned `GlobalEntityId`)
        // rather than spawning a fresh anonymous entity. This makes the joint
        // — and the `angle` port `lunco-cosim` auto-exposes on any
        // `RevoluteJoint` — addressable by USD path, API id, or `Entity` alike,
        // so the wiring fabric can target `</…/Joint>.angle` with no
        // USD-specific lookup.
        let attached = match pending.joint_type.as_str() {
            "PhysicsPrismaticJoint" => {
                let mut joint = PrismaticJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_local_basis1(pending.local_rot0)
                    .with_local_basis2(pending.local_rot1)
                    .with_slider_axis(pending.axis)
                    .with_limits(pending.limit_lower, pending.limit_upper);
                if let Some(d) = pending.drive {
                    joint.motor = LinearMotor {
                        enabled: d.is_active(),
                        target_position: d.target_position.unwrap_or(0.0),
                        target_velocity: d.target_velocity.unwrap_or(0.0),
                        max_force: d.max_force.unwrap_or(JOINT_DRIVE_MAX_FORCE_DEFAULT),
                        motor_model: resolved_drive_model
                            .expect("resolved USD prismatic drive motor"),
                    };
                }
                attach_joint(
                    &mut commands,
                    joint_entity,
                    b0,
                    b1,
                    JointSpec::new(joint).with_usd_seat(pending),
                );
                true
            }
            "PhysicsRevoluteJoint" => {
                let mut joint = RevoluteJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_local_basis1(pending.local_rot0)
                    .with_local_basis2(pending.local_rot1)
                    .with_hinge_axis(pending.axis)
                    .with_angle_limits(pending.limit_lower, pending.limit_upper);
                if let Some(d) = pending.drive {
                    joint.motor = AngularMotor {
                        enabled: d.is_active(),
                        target_position: d.target_position.unwrap_or(0.0),
                        target_velocity: d.target_velocity.unwrap_or(0.0),
                        max_torque: d.max_force.unwrap_or(JOINT_DRIVE_MAX_FORCE_DEFAULT),
                        motor_model: resolved_drive_model
                            .expect("resolved USD revolute drive motor"),
                    };
                }
                attach_joint(
                    &mut commands,
                    joint_entity,
                    b0,
                    b1,
                    JointSpec::new(joint).with_usd_seat(pending),
                );
                true
            }
            "PhysicsFixedJoint" => {
                attach_joint(
                    &mut commands,
                    joint_entity,
                    b0,
                    b1,
                    JointSpec::new(
                        FixedJoint::new(b0, b1)
                            .with_local_anchor1(pending.local_pos0)
                            .with_local_anchor2(pending.local_pos1)
                            .with_local_basis1(pending.local_rot0)
                            .with_local_basis2(pending.local_rot1),
                    )
                    .with_usd_seat(pending),
                );
                true
            }
            "PhysicsSphericalJoint" => {
                // Ball joint: 3 rotational DOF about the anchor. `physics:axis`
                // is the twist axis; the cone (`physics:coneAngle*Limit`) bounds
                // swing, `physics:limit{Lower,Upper}` bounds twist. Suspension
                // uprights, robotic wrists, gimbals.
                let mut joint = SphericalJoint::new(b0, b1)
                    .with_local_anchor1(pending.local_pos0)
                    .with_local_anchor2(pending.local_pos1)
                    .with_local_basis1(pending.local_rot0)
                    .with_local_basis2(pending.local_rot1)
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
                attach_joint(
                    &mut commands,
                    joint_entity,
                    b0,
                    b1,
                    JointSpec::new(joint).with_usd_seat(pending),
                );
                true
            }
            "PhysicsDistanceJoint" => {
                // Tether/strut: keeps the two anchors within [min, max] distance.
                // Cables, fixed-length links. A NEGATIVE (or unauthored) distance
                // is the schema's "this bound is disabled" sentinel — a disabled
                // max leaves the tether free beyond min, never a rigid rod.
                let min = if pending.limit_lower.is_finite() {
                    pending.limit_lower.max(0.0)
                } else {
                    0.0
                };
                let max = if pending.limit_upper.is_finite() && pending.limit_upper >= 0.0 {
                    pending.limit_upper.max(min)
                } else {
                    f64::INFINITY
                };
                attach_joint(
                    &mut commands,
                    joint_entity,
                    b0,
                    b1,
                    JointSpec::new(
                        DistanceJoint::new(b0, b1)
                            .with_local_anchor1(pending.local_pos0)
                            .with_local_anchor2(pending.local_pos1)
                            .with_limits(min, max),
                    )
                    .with_usd_seat(pending),
                );
                true
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
                false
            }
            other => {
                warn!("Unsupported USD joint type: {}", other);
                false
            }
        };

        // JointDamping must live on the same entity as the Avian joint. The
        // joint itself is still parked until both bodies enter the island graph;
        // inserting this carrier now means the damping is present from the
        // first constrained velocity solve, with no startup frame gap.
        if attached {
            if let Some(damping) = pending.damping {
                commands.entity(joint_entity).try_insert(damping);
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
/// THE ONLY way to hand an Avian joint to the world. Every joint in this
/// workspace — authored USD joints here, the synthesized wheel joint in
/// `lunco-usd-sim` — goes through this, and nothing else may insert a joint
/// component. It takes the two BODIES as arguments precisely so it can enforce
/// what a bare bundle could not.
///
/// It makes TWO avian rules un-forgettable, because a caller can no longer state
/// either one:
///
/// 1. **A jointed pair never reaches the narrow phase.** `JointCollisionDisabled`
///    rides the same bundle as the joint component (never a later insert), and
///    the pair is entered into [`filtered_pairs::filter_pair`] the moment it is
///    attached — so no contact can form even while the joint is still parked.
/// 2. **A joint may only enter the graph once BOTH bodies are in avian's island
///    graph.** The joint is parked as a [`PendingJoint`] and installed by
///    [`admit_pending_joints`] on the first tick where that holds.
///
/// The two construction paths share this entry point, so no caller can bypass
/// the island-admission gate or request immediate installation.
///
/// Why the bundle, specifically. Bevy writes a whole bundle before firing any
/// hook or observer, so `add_joint_to_graph` (`joint_graph/plugin.rs:135-143`)
/// reads `Has<JointCollisionDisabled> == true` and the `JointGraphEdge` is born
/// with collision disabled. The broad phase therefore never creates a contact
/// pair for the jointed bodies.
///
/// The bundle must land before the first narrow phase that could put its bodies
/// in contact. The admission gate and each caller's startup ordering establish
/// that timing.
pub fn attach_joint<J: Component + Clone>(
    commands: &mut Commands,
    joint_entity: Entity,
    body0: Entity,
    body1: Entity,
    joint: JointSpec<J>,
) {
    let JointSpec { joint, seat } = joint;
    // Rule 1, and it lands NOW rather than with the joint: a jointed pair must
    // never reach the narrow phase, and a contact formed during the wait cannot
    // be cleaned up afterwards without corrupting avian's island bookkeeping.
    // See `filtered_pairs::filter_pair`.
    filtered_pairs::filter_pair(commands, body0, body1);
    commands.entity(joint_entity).try_insert((
        PendingJoint {
            body0,
            body1,
            joint,
            seat,
        },
        PendingJointAdmission { body0, body1 },
        lunco_physics::PhysicsJointLink { body0, body1 },
        lunco_physics::PhysicsJointPending,
    ));
}

/// The other half of [`attach_joint`]: installs parked joints once their bodies
/// are admitted. **An app that attaches joints must add this**, or they park
/// forever.
///
/// A plugin rather than five `add_systems` lines at the call site, because the
/// set of joint kinds is this crate's knowledge and nobody else should have to
/// restate it — including the tests, which is where a restated list silently
/// drifts (a test app missing one kind proves nothing about that kind).
/// [`UsdAvianPlugin`] adds it; a plain-avian harness adds it directly.
pub struct JointAttachPlugin;

/// The systems that install parked joints.
///
/// Public so a joint BUILDER can order itself before them. Admission runs in
/// the outer `Update` schedule because that schedule continues while the
/// nested Avian physics schedule is held for scene readiness. The builder seats
/// the joint in `FixedPostUpdate`; the deferred command boundary then exposes
/// the parked constraint to this set, and the next fixed physics step consumes
/// the admitted component. The readiness hold keeps the seated assembly from
/// integrating during that boundary.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct JointAdmission;

impl Plugin for JointAttachPlugin {
    fn build(&self, app: &mut App) {
        // One registration per joint type: the ticket is generic over the
        // constraint it carries, so a new joint kind is one line HERE and
        // nothing else anywhere.
        //
        // Admission is structural topology work, not solver work. It must be
        // able to run while the nested PhysicsSchedule is paused by the world
        // readiness hold; otherwise the hold waits for PendingJointAdmission
        // while the only system that can clear it is itself paused. Body island
        // nodes are already authoritative by this point, and Avian consumes
        // the installed constraint on the next fixed physics step.
        app.add_systems(
            Update,
            (
                admit_pending_joints::<RevoluteJoint>
                    .run_if(any_with_component::<PendingJoint<RevoluteJoint>>),
                admit_pending_joints::<PrismaticJoint>
                    .run_if(any_with_component::<PendingJoint<PrismaticJoint>>),
                admit_pending_joints::<FixedJoint>
                    .run_if(any_with_component::<PendingJoint<FixedJoint>>),
                admit_pending_joints::<SphericalJoint>
                    .run_if(any_with_component::<PendingJoint<SphericalJoint>>),
                admit_pending_joints::<DistanceJoint>
                    .run_if(any_with_component::<PendingJoint<DistanceJoint>>),
            )
                .in_set(JointAdmission),
        );
    }
}

/// A constructed constraint that is not yet a component — the only currency
/// [`attach_joint`] accepts, and the only thing a joint builder hands back.
///
/// This is the compile-time half of the contract. The inner value is private, so
/// outside this module a `JointSpec` cannot be unwrapped, and `JointSpec` itself
/// is not a `Component`, so it cannot be handed to `insert`/`spawn`. A caller in
/// another crate therefore has no expressible way to put a joint into the world
/// except through [`attach_joint`] — the ordering rules are not documentation it
/// must remember, they are the only path the type system leaves open.
///
/// Within this module the wrapper is transparent, because this is where joints
/// are built; the guard is against a SECOND attachment site appearing elsewhere,
/// which is exactly how the wheel joint came to bypass the admission gate.
pub struct JointSpec<J: Component + Clone> {
    joint: J,
    seat: Option<JointSeat>,
}

impl<J: Component + Clone> JointSpec<J> {
    /// Wrap a constructed constraint. Private to this crate: a joint is built by
    /// one of the builders here, never assembled by a caller.
    pub(crate) fn new(joint: J) -> Self {
        Self { joint, seat: None }
    }

    fn with_seat(mut self, seat: JointSeat) -> Self {
        self.seat = Some(seat);
        self
    }

    fn with_usd_seat(self, pending: &PendingUsdJoint) -> Self {
        match JointSeat::usd(pending) {
            Some(seat) => self.with_seat(seat),
            None => self,
        }
    }
}

/// A joint that has been handed to [`attach_joint`] and is waiting for avian to
/// admit both of its bodies. Insert only through that function.
///
/// This type is what makes the two rules structural instead of remembered: the
/// bundle is assembled in ONE place ([`admit_pending_joints`]) and it is
/// assembled only once both bodies are in the island graph. A caller cannot get
/// the ordering wrong because a caller no longer expresses the ordering.
#[derive(Component, Clone, Debug)]
pub struct PendingJoint<J: Component + Clone> {
    /// First jointed body.
    pub body0: Entity,
    /// Second jointed body.
    pub body1: Entity,
    /// The constraint to install once both bodies are admitted.
    pub joint: J,
    /// Common authored-frame seating contract, when this joint has one.
    seat: Option<JointSeat>,
}

/// Cross-kind lifecycle marker for a joint parked by [`attach_joint`].
///
/// [`PendingJoint`] carries the typed constraint, so a consumer that needs to
/// ask whether *any* native joint is still waiting would otherwise have to
/// duplicate the complete list of Avian joint types. The endpoints are kept on
/// this marker so the dynamic-admission gate can hold only the bodies belonging
/// to this still-pending constraint; already-admitted parts of an articulated
/// assembly must not drift while an unrelated joint catches up.
#[derive(Component, Clone, Copy, Debug)]
pub struct PendingJointAdmission {
    /// First jointed body.
    pub body0: Entity,
    /// Second jointed body.
    pub body1: Entity,
}

/// Install every [`PendingJoint<J>`] whose two bodies avian has admitted into
/// its island graph, as one bundle with [`JointCollisionDisabled`].
///
/// `BodyIslandNode` is the precondition stated exactly: it is avian's own record
/// that a body is in the island graph, and it is what the joint-add path asserts
/// when it merges the two bodies' islands. Asking anything else — "does it have
/// `RigidBody`", "does it have `Position`" — approximates it and gets a body
/// that exists but is not admitted: freshly spawned (avian initialises bodies in
/// its own schedule, several frames after the USD build queues them) or disabled
/// (`lunco_physics`'s readiness freeze holds a vehicle whose model is still
/// compiling). Both cases panic in `merge_islands`.
///
/// **A STATIC body is admitted by construction.** Islands exist to manage
/// simulation and sleep for bodies the solver integrates, so avian never gives a
/// `RigidBody::Static` a `BodyIslandNode` — and demanding one of both endpoints
/// meant a joint anchored to static geometry waited for a component that would
/// never arrive. Forever, and silently: there is no terminal state and nothing
/// logs.
///
/// That is not a corner case, it is how every mounted mechanism attaches to
/// fixed infrastructure. A comms mast's dish, a dish on a tower, a hinge on a
/// habitat — all of them are a dynamic link jointed to something that does not
/// move. It is the real reason `components/comms/antenna.usda` never tracked
/// Earth on `structures/comms_mast.usda`: that mount's `body0` was the tower, a
/// standalone static collider. The namespace the joint was authored in, which is
/// where that bug was first hunted, had nothing to do with it.
///
/// At least one endpoint must still be a genuine island member: avian's
/// `merge_islands` asserts on a pair where *neither* body has one
/// (`islands/mod.rs`, "Neither body … is in an island"), and a joint welding two
/// pieces of static geometry constrains nothing the solver would ever integrate.
///
/// Registered per joint type by [`UsdAvianPlugin`]. A pending joint whose bodies
/// never arrive simply never installs — the same disposition as an unresolved
/// [`PendingUsdJoint`], and it dies with its scene.
pub fn admit_pending_joints<J: Component + Clone>(
    pending: Query<(Entity, &PendingJoint<J>)>,
    admitted: Query<(), With<avian3d::dynamics::solver::islands::BodyIslandNode>>,
    bodies: Query<&RigidBody>,
    mut q_pose: Query<(&mut Position, &mut Rotation)>,
    mut q_vel: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    q_authored_velocity: Query<&AuthoredInitialVelocity>,
    mut commands: Commands,
) {
    for (entity, p) in pending.iter() {
        let ready = |e: Entity| {
            admitted.contains(e) || bodies.get(e).map(RigidBody::is_static).unwrap_or(false)
        };
        if !ready(p.body0) || !ready(p.body1) {
            continue;
        }
        // Both static ⇒ nothing to solve, and avian panics on the pair.
        if !admitted.contains(p.body0) && !admitted.contains(p.body1) {
            continue;
        }
        if let Some(seat) = p.seat {
            seat_joint_bodies(
                "pending joint",
                p.body0,
                p.body1,
                seat,
                &mut q_pose,
                &mut q_vel,
                &q_authored_velocity,
                &mut commands,
            );
        }
        commands
            .entity(entity)
            .try_insert((p.joint.clone(), JointCollisionDisabled))
            .try_remove::<PendingJoint<J>>()
            .try_remove::<PendingJointAdmission>()
            .try_remove::<lunco_physics::PhysicsJointPending>();
    }
}

/// A plain weld between two bodies, anchored at their own origins.
///
/// A builder, because [`JointSpec`]'s contents are private: constructing a joint
/// is this crate's job, and every kind a caller can attach has a function here
/// that returns the spec. The USD path builds its welds with authored anchors
/// inside [`build_usd_physics_joints`]; this is the anchor-free form.
pub fn fixed_joint(body0: Entity, body1: Entity) -> JointSpec<FixedJoint> {
    JointSpec::new(FixedJoint::new(body0, body1))
}

pub fn wheel_revolute_joint(
    chassis: Entity,
    wheel: Entity,
    mount_local: DVec3,
    axle: DVec3,
) -> JointSpec<RevoluteJoint> {
    JointSpec::new(
        RevoluteJoint::new(chassis, wheel)
            .with_local_anchor1(mount_local)
            .with_local_anchor2(DVec3::ZERO)
            .with_hinge_axis(axle),
    )
    .with_seat(JointSeat {
        local_pos0: mount_local,
        local_pos1: DVec3::ZERO,
        local_rot0: DQuat::IDENTITY,
        local_rot1: DQuat::IDENTITY,
        axis: axle,
        kind: JointSeatKind::Revolute,
    })
}

/// Reads a `DVec3` attribute (e.g., `double3 xformOp:translate`) at full
/// f64 precision.
///
/// Thin DVec3 adapter over the canonical [`lunco_usd_bevy::read_vec3_f64`]
/// (the 4-branch `[f32;3]→[f64;3]→Vec<f32>→Vec<f64>` ladder). Keeping the
/// reader f64 end-to-end is what avoids the documented silent-`None`
/// "bodies launched into orbit" bug for `physics:localPos*` anchors.
fn read_vec3_attribute(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Option<DVec3> {
    lunco_usd_bevy::read_vec3_f64(reader, path, attr).map(|v| DVec3::new(v[0], v[1], v[2]))
}

/// Read a scalar only when its authored value is readable. `None` is reserved
/// for an omitted attribute, so a wrong USD type cannot become a schema default
/// at a physics boundary.
fn read_authored_real(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<f64>, ()> {
    match reader.real(path, attr) {
        Some(value) => Ok(Some(value)),
        None if !reader.has_authored_attribute(path, attr) => Ok(None),
        None => Err(()),
    }
}

/// Read an authored vector without treating a malformed value as an omitted
/// override. Physics vectors must also remain finite after the read.
fn read_authored_vec3(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<DVec3>, ()> {
    if !reader.has_authored_attribute(path, attr) {
        return Ok(None);
    }
    let value = read_vec3_attribute(reader, path, attr).ok_or(())?;
    value.is_finite().then_some(Some(value)).ok_or(())
}

/// Read an authored quaternion, rejecting wrong types, non-finite values and
/// the zero quaternion rather than silently using identity.
fn read_authored_quat(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<DQuat>, ()> {
    if !reader.has_authored_attribute(path, attr) {
        return Ok(None);
    }
    let q = reader
        .attr_value(path, attr)
        .and_then(|value| value.get::<openusd::gf::Quatf>())
        .ok_or(())?;
    let q = DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64);
    if !q.is_finite() || q.length_squared() <= f64::EPSILON {
        return Err(());
    }
    Ok(Some(q.normalize()))
}

/// Read a boolean while preserving the distinction between an omitted standard
/// default and an authored value of the wrong type.
fn read_authored_bool_or_default(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    path: &SdfPath,
    attr: &str,
    default: bool,
) -> Result<bool, ()> {
    match reader.boolean(path, attr) {
        Some(value) => Ok(value),
        None if !reader.has_authored_attribute(path, attr) => Ok(default),
        None => Err(()),
    }
}

/// Read mass, principal inertia, COM, damping, and friction from a rigid-body
/// prim and insert the corresponding Avian *override* components.
///
/// The single place `physics:mass`/damping/friction and the **G2 load-time**
/// mass-properties (`physics:diagonalInertia` / `physics:centerOfMass`) are read,
/// so every body gets them the same way.
///
/// An authored mass is an override; when it is omitted Avian computes total mass
/// from the collider tree and density. Inertia/COM are likewise inserted only
/// when explicitly authored. These are the same override components the runtime
/// mass-props cosim ports write (`lunco-cosim`), so authored and model-driven
/// values share one path.
fn apply_rigid_body_mass_props(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(), ()> {
    // Each of `Mass` / `AngularInertia` / `CenterOfMass` is only an OVERRIDE if the
    // matching `NoAuto*` marker is present. Without it Avian recomputes the
    // `Computed*` component from collider geometry and density and throws the
    // authored value away — see `MassPropertyHelper::update_mass_properties`, where
    // the authored component is read ONLY inside `if no_auto_*`. These markers were
    // missing, so `physics:mass`, `physics:diagonalInertia` and `physics:centerOfMass`
    // were all silently inert, as were the `lunco-cosim` mass-props write ports that
    // set the same components.
    //
    // The angular inertia is what made this expensive: the descent lander carried
    // `physics:mass = 2000` and ran with the inertia of the ~69 kg body its collider
    // volume implies at default density — measured `inertia_xx` 159 kg m^2 against
    // the 4625 its hull geometry gives. ~29x too easy to spin, so every disturbance
    // torque was amplified ~29x and the vehicle was thrown to 25 rad/s by the weld to
    // its rover before any stabiliser could answer.
    //
    // Note the interaction, which is why `NoAutoMass` alone fixes the common case:
    // with mass authored and inertia NOT authored, Avian runs
    // `set_mass(mass, /*update_angular_inertia*/ !no_auto_inertia)` — it RESCALES the
    // collider-derived inertia to the authored mass. So a body that authors only
    // `physics:mass` still gets a consistent tensor, which is the UsdPhysics
    // expectation.
    // `NoAutoMass` goes on ONLY when the mass was actually authored. An omitted
    // mass stays automatic, exactly as the UsdPhysics/Avian contract specifies.
    //
    // MassAPI's ZERO is a sentinel, not a value: `mass = 0`, `density = 0` and
    // `diagonalInertia = (0,0,0)` all mean "unauthored — compute me". Treating
    // them as overrides hands the solver a degenerate body.
    let conv = lunco_usd_bevy::stage_convention(reader).map_err(|_| ())?;
    let mpu = conv.length(1.0);
    if !mpu.is_finite() || mpu <= 0.0 {
        return Err(());
    }
    let mass = match read_authored_real(reader, sdf_path, ptok::A_MASS)? {
        None | Some(0.0) => None,
        Some(value) if value.is_finite() && value > 0.0 && value <= f32::MAX as f64 => {
            Some(value as f32)
        }
        Some(_) => return Err(()),
    };
    let body_density = match read_authored_real(reader, sdf_path, ptok::A_DENSITY)? {
        None | Some(0.0) => None,
        Some(value) if value.is_finite() && value > 0.0 => Some(value),
        Some(_) => return Err(()),
    };
    let diagonal_inertia = match read_authored_vec3(reader, sdf_path, ptok::A_DIAGONAL_INERTIA)? {
        None => None,
        Some(value) if value == DVec3::ZERO => None,
        Some(value) if value.x > 0.0 && value.y > 0.0 && value.z > 0.0 => Some(value),
        Some(_) => return Err(()),
    };
    let principal_axes = read_authored_quat(reader, sdf_path, ptok::A_PRINCIPAL_AXES)?;
    let center_of_mass = read_authored_vec3(reader, sdf_path, ptok::A_CENTER_OF_MASS)?;
    let linear_damping = match read_authored_real(reader, sdf_path, PHYSX_LINEAR_DAMPING)? {
        None => None,
        Some(value) if value.is_finite() && value >= 0.0 => Some(value),
        Some(_) => return Err(()),
    };
    let angular_damping = match read_authored_real(reader, sdf_path, PHYSX_ANGULAR_DAMPING)? {
        None => None,
        Some(value) if value.is_finite() && value >= 0.0 => Some(value),
        Some(_) => return Err(()),
    };
    let authored_linear = match read_authored_vec3(reader, sdf_path, ptok::A_VELOCITY)? {
        Some(vel) => {
            let vel = local_vector_to_world(reader, sdf_path, conv.point_d(vel)).map_err(|_| ())?;
            if !vel.is_finite() {
                return Err(());
            }
            Some(vel)
        }
        None => None,
    };
    let authored_angular = match read_authored_vec3(reader, sdf_path, ptok::A_ANGULAR_VELOCITY)? {
        Some(ang) => {
            let ang = local_vector_to_world(
                reader,
                sdf_path,
                conv.dir_d(ang) * std::f64::consts::PI / 180.0,
            )
            .map_err(|_| ())?;
            if !ang.is_finite() {
                return Err(());
            }
            Some(ang)
        }
        None => None,
    };
    let material_density = read_physics_material(reader, sdf_path)
        .map_err(|_| ())?
        .and_then(|pm| pm.density)
        .filter(|d| *d > 0.0)
        .map(f64::from);
    let collider_density = if let Some(density) = body_density.or(material_density) {
        let collider_density = density / (mpu * mpu * mpu);
        if !collider_density.is_finite() || collider_density > f32::MAX as f64 {
            return Err(());
        }
        Some(collider_density as f32)
    } else {
        None
    };

    if let Some(mass) = mass {
        commands.entity(entity).try_insert((Mass(mass), NoAutoMass));
    }

    // `physics:density` — on the body's MassAPI, else on the bound physics
    // material — feeds avian's collider-mass derivation. Precedence is the
    // spec's: authored mass > body density > material density (an authored mass
    // still wins via `NoAutoMass` above). Stage units are mass per unit³.
    if let Some(collider_density) = collider_density {
        commands
            .entity(entity)
            .try_insert(ColliderDensity(collider_density));
    }

    // G2 — authored principal inertia. `physics:diagonalInertia` is the diagonal
    // of the inertia tensor in the principal frame, `physics:principalAxes` (a
    // quat, identity when unauthored) rotates that frame. Off-diagonal inertia is
    // not representable here (Avian stores principal + frame), matching the
    // UsdPhysics schema. Units are mass · distance², and the diagonal permutes
    // with the stage's axes exactly as a direction does.
    if let Some(diag) = diagonal_inertia {
        let local_frame = principal_axes
            .map(|q| conv.rotation_d(q).as_quat())
            .unwrap_or(Quat::IDENTITY);
        let principal = (conv.dir_d(diag).abs() * (mpu * mpu)).as_vec3();
        if !principal.is_finite()
            || principal.x <= 0.0
            || principal.y <= 0.0
            || principal.z <= 0.0
            || !local_frame.is_finite()
        {
            return Err(());
        }
        commands.entity(entity).try_insert((
            AngularInertia {
                principal,
                local_frame,
            },
            NoAutoAngularInertia,
        ));
    }

    // G2 — authored centre of mass (body-frame offset, a POINT in stage units).
    if let Some(com) = center_of_mass {
        let com = conv.point_d(com).as_vec3();
        if !com.is_finite() {
            return Err(());
        }
        commands
            .entity(entity)
            .try_insert((CenterOfMass(com), NoAutoCenterOfMass));
    }

    if let Some(d) = linear_damping {
        commands.entity(entity).try_insert(LinearDamping(d));
    }
    if let Some(d) = angular_damping {
        commands.entity(entity).try_insert(AngularDamping(d));
    }
    apply_physics_material(commands, entity, reader, sdf_path)?;
    // The spec frames both velocities in the BODY's local space: convert the
    // components by the stage convention (`physics:velocity` is units/s so it
    // scales like a point; `physics:angularVelocity` is DEG/s about local axes),
    // then carry them into the world frame through the body's composed rotation
    // — avian's velocity components are world-frame.
    if authored_linear.is_some() || authored_angular.is_some() {
        commands.entity(entity).try_insert(AuthoredInitialVelocity {
            linear: authored_linear,
            angular: authored_angular,
        });
    }
    Ok(())
}

/// Project the surface properties of the USD physics material bound to a prim.
///
/// This is intentionally shared by static terrain and dynamic rigid bodies:
/// material binding describes a surface, not a mobility class. A terrain
/// classifier that skipped this projection would silently turn authored lunar
/// regolith into Avian's default surface and make touchdown behavior depend on
/// which USD prim type happened to carry the collider.
fn apply_physics_material(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(), ()> {
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
    // The pairwise combination remains Avian's responsibility. USD describes
    // each surface; it does not average the two surfaces at load time.
    if let Some(pm) = read_physics_material(reader, sdf_path).map_err(|_| ())? {
        if pm.dynamic_friction.is_some() || pm.static_friction.is_some() {
            let d = Friction::default();
            let friction = Friction {
                dynamic_coefficient: pm
                    .dynamic_friction
                    .map_or(d.dynamic_coefficient, |f| f.into()),
                static_coefficient: pm
                    .static_friction
                    .map_or(d.static_coefficient, |f| f.into()),
                combine_rule: pm.friction_combine.unwrap_or(d.combine_rule),
            };
            commands.entity(entity).try_insert(friction);

            // Avian snapshots `ActiveCollisionHooks` when it creates the
            // collider-tree proxy.  Adding MODIFY_CONTACTS later through a
            // Changed<Friction> system updates only the broad-phase filter bit
            // in Avian 0.7, not the contact-modification bit.  Arm this hook at
            // the same load-time boundary as the material, before the collider
            // is admitted, so authored static friction is actually part of the
            // contact contract for compound and standalone surfaces alike.
            if (friction.static_coefficient - friction.dynamic_coefficient).abs()
                > avian3d::math::Scalar::EPSILON
            {
                filtered_pairs::enable_collision_hook(
                    commands,
                    entity,
                    ActiveCollisionHooks::MODIFY_CONTACTS,
                );
            }
        }
        if let Some(r) = pm.restitution {
            let d = Restitution::default();
            commands.entity(entity).try_insert(Restitution {
                coefficient: r.into(),
                combine_rule: pm.restitution_combine.unwrap_or(d.combine_rule),
            });
        }
    }
    Ok(())
}

/// Damping is **not** a UsdPhysics concept — the core spec has no damping
/// attribute at all. Omniverse contributes it via `PhysxRigidBodyAPI`, and these
/// are its names. `physics:*Damping` is not a valid spelling: it would squat the
/// UsdPhysics namespace with an attribute that spec does not define.
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
fn combine_mode(token: &str) -> Option<CoefficientCombine> {
    match token {
        "average" => Some(CoefficientCombine::Average),
        "min" => Some(CoefficientCombine::Min),
        "multiply" => Some(CoefficientCombine::Multiply),
        "max" => Some(CoefficientCombine::Max),
        _ => None,
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
    /// `physics:density` — for bodies that author no mass of their own (stage
    /// units: mass per unit³).
    pub density: Option<f32>,
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
/// Friction comes off the bound `Material`, never off a bare `physics:friction`
/// on the body prim: that name is not defined by UsdPhysics, so no other
/// physics-aware consumer reads it, and USD is free to give it another meaning.
///
/// Binding resolution — namespace inheritance, and the purpose→all-purpose
/// fallback that lets ONE `Material` drive both look and friction — is SHARED
/// with the renderer ([`lunco_usd_bevy::resolve_bound_material`]). A physical and
/// a visual material are the same USD concept bound for different purposes, so
/// they must resolve through the same code or they will drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicsMaterialReadError {
    /// Authored material attribute that failed strict decoding.
    pub attribute: String,
}

impl PhysicsMaterialReadError {
    fn new(attribute: &str) -> Self {
        Self {
            attribute: attribute.to_owned(),
        }
    }
}

impl std::fmt::Display for PhysicsMaterialReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid authored physics material `{}`", self.attribute)
    }
}

impl std::error::Error for PhysicsMaterialReadError {}

pub fn read_physics_material(
    reader: &dyn lunco_usd_bevy::read::UsdReadObject,
    prim: &SdfPath,
) -> Result<Option<PhysicsMaterial>, PhysicsMaterialReadError> {
    use openusd::schemas::physics::tokens as ptok;

    let Some(mat_path) = reader.bound_material(prim, lunco_usd_bevy::MaterialPurpose::Physics)
    else {
        return Ok(None);
    };
    let mat = SdfPath::new(&mat_path)
        .map_err(|_| PhysicsMaterialReadError::new(ptok::REL_MATERIAL_BINDING_PHYSICS))?;
    if !reader.has_api_schema(&mat, "PhysicsMaterialAPI") {
        return Ok(None);
    }
    let read_coefficient =
        |attr: &str, upper: Option<f64>| -> Result<Option<f32>, PhysicsMaterialReadError> {
            match read_authored_real(reader, &mat, attr)
                .map_err(|_| PhysicsMaterialReadError::new(attr))?
            {
                None => Ok(None),
                Some(value)
                    if value.is_finite()
                        && value >= 0.0
                        && upper.is_none_or(|maximum| value <= maximum)
                        && value <= f32::MAX as f64 =>
                {
                    Ok(Some(value as f32))
                }
                Some(_) => Err(PhysicsMaterialReadError::new(attr)),
            }
        };
    let dynamic_friction = read_coefficient(ptok::A_DYNAMIC_FRICTION, None)?;
    let static_friction = read_coefficient(ptok::A_STATIC_FRICTION, None)?;
    let restitution = read_coefficient(ptok::A_RESTITUTION, Some(1.0))?;
    let density = read_coefficient(ptok::A_DENSITY, None)?;
    let read_combine =
        |attr: &str| -> Result<Option<CoefficientCombine>, PhysicsMaterialReadError> {
            if !reader.has_authored_attribute(&mat, attr) {
                return Ok(None);
            }
            let token = match reader.attr_value(&mat, attr) {
                Some(openusd::sdf::Value::Token(token)) => token.to_string(),
                _ => return Err(PhysicsMaterialReadError::new(attr)),
            };
            Ok(Some(
                combine_mode(&token).ok_or_else(|| PhysicsMaterialReadError::new(attr))?,
            ))
        };
    let friction_combine = read_combine(PHYSX_FRICTION_COMBINE_MODE)?;
    let restitution_combine = read_combine(PHYSX_RESTITUTION_COMBINE_MODE)?;

    // A Material bound only for LOOKS resolves here via the purpose→all-purpose
    // fallback but carries no `PhysicsMaterialAPI` properties. That is not a
    // physics material — don't fabricate a zero-friction one out of it.
    Ok((dynamic_friction.is_some()
        || static_friction.is_some()
        || restitution.is_some()
        || density.is_some())
    .then_some(PhysicsMaterial {
        dynamic_friction,
        static_friction,
        restitution,
        density,
        friction_combine,
        restitution_combine,
    }))
}

/// Marker component to hold a rigid body as Kinematic until all joints
/// and constraints are fully resolved in the stage, preventing 1-frame
/// physics separation explosions.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShouldBeDynamic;

/// Initial velocity authored on a USD body that is waiting for joint admission.
///
/// Avian integrates a kinematic body's `LinearVelocity` as a commanded motion,
/// so merely changing the body type to `Kinematic` does not freeze a body that
/// already received `physics:velocity`. Dynamic bodies therefore keep their
/// authored initial condition here until the USD simulation projector promotes
/// them. This component is the lifecycle contract between USD extraction and
/// dynamic admission; it is not a model-specific workaround.
#[derive(Component, Clone, Copy, Debug)]
pub struct AuthoredInitialVelocity {
    /// World-frame linear velocity, if authored.
    pub linear: Option<DVec3>,
    /// World-frame angular velocity, if authored.
    pub angular: Option<DVec3>,
}

// USDA fixtures are written to a temp dir and composed from disk. Native-only
// test code: the `disallowed_methods` ban on `std::fs` guards wasm *runtime*
// paths (clippy.toml names `tests/` as exempt; cargo has no path-scoped lint
// config, so the exemption is written out).
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)]
mod collider_parity_tests {
    //! The collider read path, driven off the
    //! live `StageView` over the canonical stage. Exercises the geometry read
    //! (the highest-risk physics read), including the mesh-approximation selector.

    use super::build_collider_from_usd;
    use bevy::math::DVec3;
    use lunco_usd_bevy::{StageView, compose_file_to_stage};
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
        }\n\
        def Mesh \"BadHull\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\", \"PhysicsMeshCollisionAPI\"] )\n{\n\
            point3f[] points = [(0,0,0),(1,0,0),(2,0,0),(3,0,0)]\n\
            int[] faceVertexCounts = [3,3]\n\
            int[] faceVertexIndices = [0,1,2, 1,2,3]\n\
            uniform token physics:approximation = \"convexHull\"\n\
        }\n\
        def Mesh \"BoundingCube\" ( prepend apiSchemas = [\"PhysicsCollisionAPI\", \"PhysicsMeshCollisionAPI\"] )\n{\n\
            point3f[] points = [(0,0,0),(2,0,0),(2,2,0),(0,2,0),(1,1,2)]\n\
            int[] faceVertexCounts = [3,3,3,3]\n\
            int[] faceVertexIndices = [0,1,4, 1,2,4, 2,3,4, 3,0,4]\n\
            uniform token physics:approximation = \"boundingCube\"\n\
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
            .expect("valid transform")
            .expect("default mesh → trimesh collider");
        let hull = build_collider_from_usd(&view, &SdfPath::new("/Hull").unwrap())
            .expect("valid transform")
            .expect("convexHull approximation → collider");
        assert_ne!(
            format!("{trimesh:?}"),
            format!("{hull:?}"),
            "`physics:approximation = convexHull` must build a DIFFERENT collider than the default trimesh"
        );
        assert!(
            build_collider_from_usd(&view, &SdfPath::new("/BadHull").unwrap())
                .expect("valid transform")
                .is_none(),
            "a failed authored convex hull must not silently become a triangle mesh"
        );
        assert!(
            build_collider_from_usd(&view, &SdfPath::new("/BoundingCube").unwrap())
                .expect("valid transform")
                .is_none(),
            "an unsupported authored approximation must not silently become a triangle mesh"
        );
    }

    #[test]
    fn collider_uses_composed_named_scale_and_rejects_malformed_scale() {
        const SOURCE: &str = r#"#usda 1.0
def Cube "Scaled" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
{
    double size = 2
    double3 xformOp:scale:wide = (2, 3, 4)
    uniform token[] xformOpOrder = ["xformOp:scale:wide"]
}
def Cube "Malformed" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
{
    uniform token[] xformOpOrder = ["xformOp:scale:missing"]
}
"#;
        let dir = std::env::temp_dir().join("lunco_collider_composed_scale");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("scale.usda");
        std::fs::write(&f, SOURCE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);

        let scaled = build_collider_from_usd(&view, &SdfPath::new("/Scaled").unwrap())
            .expect("named scale is a valid composed transform")
            .expect("scaled cube collider");
        assert_eq!(scaled.scale(), DVec3::new(2.0, 3.0, 4.0));

        let error = build_collider_from_usd(&view, &SdfPath::new("/Malformed").unwrap())
            .expect_err("a transform that names a missing scale op must be rejected");
        assert_eq!(error.prim, "/Malformed");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)] // temp-dir USDA fixtures; see `collider_parity_tests`
mod extract_parity_tests {
    //! End-to-end physics extraction off the live `StageView`: the REAL
    //! `extract_avian_prim` on a rover chassis with a child collider at an
    //! authored transform, exercising the whole read layer (schema detect →
    //! compound collider → `collect_child_colliders` → `local_transform_at`
    //! → `local_transform_at` → mass props).

    use super::{CollisionGroupTable, extract_avian_prim, read_physics_material};
    use avian3d::prelude::*;
    use bevy::ecs::world::CommandQueue;
    use bevy::prelude::*;
    use lunco_usd_bevy::{StageView, compose_file_to_stage};
    use openusd::sdf::Path as SdfPath;

    // A rover chassis (RigidBodyAPI, mass 500) with a child Cube collider
    // (CollisionAPI) offset by an authored xformOp:translate — the compound path.
    const FIXTURE: &str = "#usda 1.0\n\ndef Xform \"Rover\" (\n    prepend apiSchemas = [\"PhysicsRigidBodyAPI\"]\n)\n{\n    double physics:mass = 500\n    def Cube \"Body\" (\n        prepend apiSchemas = [\"PhysicsCollisionAPI\"]\n    )\n    {\n        double size = 2\n        double3 xformOp:translate = (0, 1, 0)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";

    const MATERIAL_FIXTURE: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "World"
{
    def Scope "PhysicsMaterials"
    {
        def Material "Regolith" ( prepend apiSchemas = ["PhysicsMaterialAPI"] )
        {
            float physics:dynamicFriction = 0.7
            float physics:staticFriction = 0.9
            float physics:restitution = 0.2
            token physxMaterial:frictionCombineMode = "min"
        }
    }
    def Cube "Ground" (
        prepend apiSchemas = ["PhysicsCollisionAPI", "MaterialBindingAPI"]
    )
    {
        double size = 4
        rel material:binding:physics = </World/PhysicsMaterials/Regolith>
    }
}
"#;

    /// Run `extract_avian_prim` on a fresh world and read back the physics the
    /// chassis received: (body type, collider Debug, mass, has ShouldBeDynamic).
    fn run_extract(
        reader: &StageView<'_>,
        path: &SdfPath,
    ) -> (Option<RigidBody>, Option<String>, Option<f32>, bool) {
        let mut world = World::new();
        let e = world.spawn_empty().id();
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            extract_avian_prim(
                reader,
                e,
                path,
                &CollisionGroupTable::default(),
                &mut commands,
                None,
                None,
            );
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
        assert!(
            live.1.is_some(),
            "live: compound collider built off the stage"
        );
        assert_eq!(
            live.2,
            Some(500.0),
            "live: authored mass read off the stage"
        );
        assert!(live.3, "live: ShouldBeDynamic (settles to Dynamic)");
    }

    #[test]
    fn omitted_mass_is_left_to_avian_computed_mass() {
        let source = FIXTURE.replace("    double physics:mass = 500\n", "");
        let dir = std::env::temp_dir().join("lunco_extract_parity");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("rover_automatic_mass.usda");
        std::fs::write(&f, source).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        let view = StageView::new(&stage);

        let live = run_extract(&view, &SdfPath::new("/Rover").unwrap());

        assert_eq!(
            live.0,
            Some(RigidBody::Kinematic),
            "an omitted mass must not prevent body extraction"
        );
        assert!(
            live.1.is_some(),
            "collider remains available for Avian mass derivation"
        );
        assert_eq!(
            live.2, None,
            "the USD projector must not invent the old 1000 kg mass seed"
        );
    }

    #[test]
    fn malformed_rigid_body_mass_properties_refuse_projection() {
        let cases = [
            (
                "bad_mass",
                FIXTURE.replace(
                    "double physics:mass = 500",
                    "string physics:mass = \"not-a-mass\"",
                ),
            ),
            (
                "bad_inertia",
                FIXTURE.replace(
                    "double physics:mass = 500",
                    "double physics:mass = 500\n    float physics:diagonalInertia = 1.0",
                ),
            ),
            (
                "bad_velocity",
                FIXTURE.replace(
                    "double physics:mass = 500",
                    "double physics:mass = 500\n    string physics:velocity = \"not-a-vector\"",
                ),
            ),
        ];

        for (name, source) in cases {
            let dir = std::env::temp_dir().join("lunco_extract_parity");
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join(format!("rover_{name}.usda"));
            std::fs::write(&f, source).unwrap();
            let stage = compose_file_to_stage(&f).expect("compose stage");
            let view = StageView::new(&stage);

            let live = run_extract(&view, &SdfPath::new("/Rover").unwrap());
            assert_eq!(
                live.0, None,
                "malformed rigid-body field in {name} must not create a body"
            );
            assert_eq!(
                live.1, None,
                "malformed rigid-body field in {name} must not create a collider"
            );
            assert_eq!(
                live.2, None,
                "malformed rigid-body field in {name} must not create mass"
            );
        }
    }

    #[test]
    fn physics_material_reader_rejects_malformed_values_and_tokens() {
        let cases = [
            (
                "bad_friction",
                MATERIAL_FIXTURE.replace(
                    "float physics:dynamicFriction = 0.7",
                    "string physics:dynamicFriction = \"not-friction\"",
                ),
            ),
            (
                "bad_combine",
                MATERIAL_FIXTURE.replace(
                    "token physxMaterial:frictionCombineMode = \"min\"",
                    "token physxMaterial:frictionCombineMode = \"unknown\"",
                ),
            ),
            (
                "negative_restitution",
                MATERIAL_FIXTURE.replace(
                    "float physics:restitution = 0.2",
                    "float physics:restitution = -0.1",
                ),
            ),
        ];

        for (name, source) in cases {
            let dir = std::env::temp_dir().join("lunco_material_parity");
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join(format!("{name}.usda"));
            std::fs::write(&f, source).unwrap();
            let stage = compose_file_to_stage(&f).expect("compose material stage");
            let view = StageView::new(&stage);
            assert!(
                read_physics_material(&view, &SdfPath::new("/World/Ground").unwrap()).is_err(),
                "malformed material value in {name} must not disappear into Avian defaults"
            );
        }
    }

    #[test]
    fn standalone_static_colliders_receive_their_bound_physics_material() {
        let dir = std::env::temp_dir().join("lunco_material_parity");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("valid_material.usda");
        std::fs::write(&f, MATERIAL_FIXTURE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose material stage");
        let view = StageView::new(&stage);
        {
            let mut world = World::new();
            let entity = world.spawn_empty().id();
            let mut queue = CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, &world);
                extract_avian_prim(
                    &view,
                    entity,
                    &SdfPath::new("/World/Ground").unwrap(),
                    &CollisionGroupTable::default(),
                    &mut commands,
                    None,
                    None,
                );
            }
            queue.apply(&mut world);
            let friction = world
                .get::<Friction>(entity)
                .copied()
                .expect("static ground receives its physics material");
            assert!((friction.dynamic_coefficient - 0.7).abs() < 1e-6);
            assert!((friction.static_coefficient - 0.9).abs() < 1e-6);
            assert_eq!(friction.combine_rule, CoefficientCombine::Min);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)] // temp-dir USDA fixtures; see `collider_parity_tests`
mod joint_reader_tests {
    //! The joint projector reads the STANDARD UsdPhysics joint schema through
    //! the composed reader into the
    //! deferred `PendingUsdJoint` — bodies, axis, standard `physics:lowerLimit`/
    //! `upperLimit` (degrees → radians), local anchors, and `UsdPhysicsDriveAPI`.
    //! This is the headless-verifiable half of the rework (the read); joint
    //! *dynamics* need a rover boot.
    use super::{read_joint_spec, read_joint_spec_for_lint};
    use avian3d::prelude::MotorModel;
    use bevy::math::DVec3;
    use lunco_usd_bevy::{StageView, compose_file_to_stage};
    use openusd::sdf::Path as SdfPath;

    const FIXTURE: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsRevoluteJoint "Hinge" (
    prepend apiSchemas = ["PhysicsDriveAPI:angular", "LunCoJointDampingAPI"]
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
    float lunco:jointDamping:angular = 2.5
}
"#;

    #[test]
    fn reads_standard_revolute_joint_off_live_stage() {
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("hinge.usda");
        std::fs::write(&f, FIXTURE).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");

        let j = read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Hinge").unwrap())
            .expect("standard revolute joint reads through the composed reader");

        assert_eq!(j.joint_type, "PhysicsRevoluteJoint");
        assert_eq!(j.body0_path, "/Chassis");
        assert_eq!(j.body1_path, "/Wheel");
        assert_eq!(j.axis, DVec3::Y);
        // Standard `physics:lowerLimit`/`upperLimit` are DEGREES → radians.
        assert!(
            (j.limit_lower - (-45f64).to_radians()).abs() < 1e-9,
            "lower {}",
            j.limit_lower
        );
        assert!(
            (j.limit_upper - 45f64.to_radians()).abs() < 1e-9,
            "upper {}",
            j.limit_upper
        );
        assert_eq!(j.local_pos0, DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(j.local_pos1, DVec3::ZERO);
        // UsdPhysicsDriveAPI:angular → JointDrive.
        let drive = j.drive.expect("angular drive read via DriveAPI");
        assert_eq!(drive.target_velocity, Some(2.5f64.to_radians()));
        assert_eq!(drive.max_force, Some(100.0));
        assert_eq!(drive.target_position, None);
        let damping = j.damping.expect("typed passive joint damping");
        assert_eq!(damping.linear, 0.0);
        assert_eq!(damping.angular, 2.5);
    }

    #[test]
    fn angular_force_drive_uses_authored_effective_inertia() {
        let source = "#usda 1.0\n\
(\n\
    metersPerUnit = 1\n\
)\n\
def Xform \"Host\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsMassAPI\"] )\n\
{\n\
    float physics:mass = 10.0\n\
    float3 physics:diagonalInertia = (100.0, 100.0, 100.0)\n\
    def Xform \"Link\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\", \"PhysicsMassAPI\"] )\n\
    {\n\
        float physics:mass = 2.0\n\
        float3 physics:diagonalInertia = (2.0, 3.0, 4.0)\n\
    }\n\
}\n\
def PhysicsRevoluteJoint \"Hinge\" ( prepend apiSchemas = [\"PhysicsDriveAPI:angular\"] )\n\
{\n\
    rel physics:body0 = </Host>\n\
    rel physics:body1 = </Host/Link>\n\
    uniform token physics:axis = \"Y\"\n\
    point3f physics:localPos0 = (1.0, 0.0, 0.0)\n\
    point3f physics:localPos1 = (1.0, 1.0, 0.0)\n\
    uniform token drive:angular:physics:type = \"force\"\n\
    float drive:angular:physics:stiffness = 300.0\n\
    float drive:angular:physics:damping = 30.0\n\
}\n";
        let stage = write_and_compose("angular_inertia.usda", source);
        let joint = read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Hinge").unwrap())
            .expect("angular force drive reads");
        let drive = joint.drive.expect("drive is authored");
        let expected_inertia = 1.0 / (1.0 / 110.0 + 1.0 / 5.0);
        assert!((drive.generalized_inertia.unwrap() - expected_inertia).abs() < 1e-9);
        assert!(matches!(
            drive.motor_model(),
            Ok(MotorModel::SpringDamper { .. })
        ));
    }

    /// Where a raked joint's axis actually POINTS, from the authoring a landing
    /// leg uses.
    ///
    /// `physics:axis` can only name X, Y or Z, so a strut raked 25° off vertical
    /// carries the rake in `physics:localRot0` and the axis is read IN that
    /// basis. Everything downstream depends on the resulting direction — most
    /// sharply the authored travel range, since a leg's `lowerLimit = -0.8,
    /// upperLimit = 0.0` is only soft if load pushes it toward NEGATIVE
    /// displacement. Point the axis the other way and the joint is not sprung at
    /// all: it jams against its upper limit at zero stroke and carries the
    /// vehicle rigidly, reporting no spring force. That reads, from outside, as
    /// "the suspension is too stiff".
    ///
    /// USD quaternions are authored `(w, x, y, z)`. `(-0.216440, 0, 0, 0.976296)`
    /// is therefore w = -0.216440, z = 0.976296: a 205° rotation about +Z, which
    /// takes the frame's +Y onto (0.42262, -0.90631, 0) — outward and DOWN, hull
    /// toward foot. This test exists because reading those four numbers in the
    /// other order (x first) is silent, plausible, and yields an axis pointing
    /// UP instead, inverting every sign the mechanism depends on.
    #[test]
    fn a_raked_joint_axis_points_where_the_quaternion_says() {
        const RAKED: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Hull" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Leg" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsPrismaticJoint "Spring" (
    prepend apiSchemas = ["PhysicsDriveAPI:linear"]
)
{
    rel physics:body0 = </Hull>
    rel physics:body1 = </Leg>
    uniform token physics:axis = "Y"
    quatf physics:localRot0 = (-0.216440, 0, 0, 0.976296)
    quatf physics:localRot1 = (0, 0, 0, 1)
    float physics:lowerLimit = -0.8
    float physics:upperLimit = 0.0
    uniform token drive:linear:physics:type = "force"
    float drive:linear:physics:stiffness = 4000.0
}
"#;
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("raked.usda");
        std::fs::write(&f, RAKED).unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");

        let j = read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Spring").unwrap())
            .expect("raked prismatic joint reads through the composed reader");

        // The axis token itself is cardinal — the rake is not in here.
        assert_eq!(j.axis, DVec3::Y);

        // …it is here, and this is the direction the mechanism actually slides
        // along: `free_axis = local_rot0 * axis`, the same product avian forms.
        let free_axis = j.local_rot0 * j.axis;
        let want = DVec3::new(0.42262, -0.90631, 0.0);
        assert!(
            (free_axis - want).length() < 1e-4,
            "a 205°-about-Z basis must take +Y to {want:?} (outward and DOWN, \
             hull toward foot), got {free_axis:?}. An axis pointing UP here means \
             the quaternion was read in the wrong component order, and every \
             leg in the fleet is jammed against `upperLimit = 0.0`."
        );
        assert!(
            free_axis.y < 0.0,
            "the strut axis must point DOWNWARD from the hull, got {free_axis:?}"
        );

        // `localRot1` is body1's half of the same frame: 180° about Z, which is
        // what lets a leg body already carrying its own 25° rake agree with a
        // 205° joint frame.
        let flipped = j.local_rot1 * DVec3::Y;
        assert!(
            (flipped - DVec3::NEG_Y).length() < 1e-4,
            "localRot1 = (0,0,0,1) is 180° about Z and must take +Y to -Y, got {flipped:?}"
        );

        // Prismatic limits are METRES and pass through unconverted — unlike a
        // revolute's degrees. A conversion here would silently scale the leg's
        // travel by 57.
        // f32 on the wire, f64 in the joint — compare at f32 precision.
        assert!(
            (j.limit_lower - -0.8).abs() < 1e-6,
            "lower {}",
            j.limit_lower
        );
        assert!(
            (j.limit_upper - 0.0).abs() < 1e-6,
            "upper {}",
            j.limit_upper
        );
    }

    #[test]
    fn non_joint_prim_reads_none() {
        let dir = std::env::temp_dir().join("lunco_joint_typed");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("nojoint.usda");
        std::fs::write(&f, "#usda 1.0\ndef Xform \"Plain\" {}\n").unwrap();
        let stage = compose_file_to_stage(&f).expect("compose stage");
        assert!(
            read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Plain").unwrap()).is_none()
        );
    }

    #[test]
    fn lint_only_joint_is_not_projected_but_is_still_read_by_linter() {
        let source = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Hull" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Link" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsPrismaticJoint "FixtureSpring" (
    prepend apiSchemas = ["PhysicsDriveAPI:linear"]
)
{
    bool lunco:lintOnly = true
    rel physics:body0 = </Hull>
    rel physics:body1 = </Link>
    uniform token physics:axis = "Y"
    uniform token drive:linear:physics:type = "force"
    float drive:linear:physics:stiffness = 4000.0
}
"#;
        let stage = lunco_usd_bevy::CanonicalStage::from_recipe(
            &lunco_usd_bevy::StageRecipe::from_source("lint_only.usda", source),
        )
        .expect("compose lint-only fixture");
        let view = stage.view();
        let path = SdfPath::new("/FixtureSpring").expect("joint path");

        assert!(
            read_joint_spec(&view, &path).is_none(),
            "a lint-only malformed joint must never enter runtime projection"
        );
        assert!(
            read_joint_spec_for_lint(&view, &path).is_some(),
            "the linter must inspect the same composed joint authoring"
        );
    }

    #[test]
    fn authored_joint_fields_do_not_degrade_to_physics_defaults() {
        let cases = [
            (
                "bad_local_rotation",
                FIXTURE.replace(
                    "point3f physics:localPos0 = (1, 0, 0)",
                    "float physics:localRot0 = 1.0\n    point3f physics:localPos0 = (1, 0, 0)",
                ),
            ),
            (
                "bad_local_position",
                FIXTURE.replace(
                    "point3f physics:localPos0 = (1, 0, 0)",
                    "float physics:localPos0 = 1.0",
                ),
            ),
            (
                "bad_axis",
                FIXTURE.replace(
                    "uniform token physics:axis = \"Y\"",
                    "uniform token physics:axis = \"diagonal\"",
                ),
            ),
            (
                "bad_limit",
                FIXTURE.replace(
                    "float physics:lowerLimit = -45",
                    "string physics:lowerLimit = \"not-a-limit\"",
                ),
            ),
            (
                "bad_drive",
                FIXTURE.replace(
                    "float drive:angular:physics:targetVelocity = 2.5",
                    "string drive:angular:physics:targetVelocity = \"not-a-velocity\"",
                ),
            ),
            (
                "bad_joint_enabled",
                FIXTURE.replace(
                    "uniform token physics:axis = \"Y\"",
                    "uniform token physics:axis = \"Y\"\n    string physics:jointEnabled = \"false\"",
                ),
            ),
        ];

        for (name, source) in cases {
            let stage = write_and_compose(&format!("{name}.usda"), &source);
            assert!(
                read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Hinge").unwrap())
                    .is_none(),
                "authored malformed field in {name} must reject the joint"
            );
        }
    }

    #[test]
    fn a_joint_with_multiple_body_targets_is_rejected() {
        const MULTIPLE_TARGETS: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsRevoluteJoint "Hinge"
{
    rel physics:body0 = [</Chassis>, </Wheel>]
    rel physics:body1 = </Wheel>
}
"#;
        let stage = write_and_compose("multiple_body_targets.usda", MULTIPLE_TARGETS);
        assert!(
            read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Hinge").unwrap()).is_none(),
            "a joint endpoint must name exactly one body target"
        );
    }

    #[test]
    fn omitted_spherical_cone_limits_remain_unlimited() {
        const SPHERICAL: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsSphericalJoint "Ball"
{
    rel physics:body0 = </Chassis>
    rel physics:body1 = </Wheel>
}
"#;
        let stage = write_and_compose("unlimited_spherical.usda", SPHERICAL);
        let joint = read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Ball").unwrap())
            .expect("spherical joint reads");
        assert_eq!(
            joint.swing_limit, None,
            "UsdPhysics negative cone defaults mean unlimited, not a one-degree cone"
        );
    }

    #[test]
    fn an_unconfigured_generic_joint_does_not_become_fixed() {
        const GENERIC: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {}
def PhysicsJoint "Generic"
{
    rel physics:body0 = </Chassis>
    rel physics:body1 = </Wheel>
}
"#;
        let stage = write_and_compose("unconfigured_generic.usda", GENERIC);
        assert!(
            read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Generic").unwrap()).is_none(),
            "an unconstrained generic joint has multiple free DOFs and cannot reduce to fixed"
        );
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

        let j = read_joint_spec(&StageView::new(&stage), &SdfPath::new("/Hinge").unwrap())
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

    const DERIVE_FIXTURE: &str = "#usda 1.0\n(\n\
    upAxis = \"Y\"\n\
    metersPerUnit = 1\n\
)\n\
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
        let j = read_joint_spec(
            &StageView::new(&stage),
            &SdfPath::new("/Rover/Hinge").unwrap(),
        )
        .expect("revolute joint reads");
        assert!(
            close(j.local_pos0, DVec3::new(0.9, -0.65, 1.225)),
            "lp0 derived from wheel translate: {:?}",
            j.local_pos0
        );
        assert_eq!(j.local_pos1, DVec3::ZERO, "lp1 = body1 origin");
    }

    #[test]
    fn authored_anchor_is_not_overridden_by_derivation() {
        // An explicit `physics:localPos0` must win — derivation fills only an
        // UNAUTHORED anchors only, so hand-tuned joints never change.
        let stage = write_and_compose(
            "authored.usda",
            &DERIVE_FIXTURE.replace(
                "AUTHORED",
                "        point3f physics:localPos0 = (1, 2, 3)\n",
            ),
        );
        let j = read_joint_spec(
            &StageView::new(&stage),
            &SdfPath::new("/Rover/Hinge").unwrap(),
        )
        .expect("revolute joint reads");
        assert_eq!(
            j.local_pos0,
            DVec3::new(1.0, 2.0, 3.0),
            "authored lp0 wins over derivation"
        );
    }

    /// A MOUNTED MECHANISM, in the shape `components/comms/antenna.usda` uses.
    ///
    /// The mechanism is a plain `Xform` (`Mount`) parented under a host body, and
    /// its own joint names THAT XFORM as `body0` — it cannot name the host, which
    /// it has never heard of. The endpoint must resolve to the nearest ancestor
    /// body, and the derived anchor must land in THAT body's frame: the mechanism
    /// sits at (0, 1, 0) on the host and its head 0.5 m above that, so lp0 is the
    /// head's origin in host coordinates, (0, 1.5, 0) — not (0, 0.5, 0), which is
    /// what resolving after the anchor derivation would produce.
    const MOUNT_FIXTURE: &str = "#usda 1.0\n(\n\
    upAxis = \"Y\"\n\
    metersPerUnit = 1\n\
)\n\
def Xform \"Host\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n{\n\
    def Xform \"Mount\"\n    {\n\
        double3 xformOp:translate = (0, 1, 0)\n\
        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n\
        def Xform \"Head\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n        {\n\
            double3 xformOp:translate = (0, 0.5, 0)\n\
            uniform token[] xformOpOrder = [\"xformOp:translate\"]\n        }\n\
        def PhysicsRevoluteJoint \"YawJoint\"\n        {\n\
            rel physics:body0 = </Host/Mount>\n\
            rel physics:body1 = </Host/Mount/Head>\n\
            uniform token physics:axis = \"Y\"\n        }\n    }\n}\n";

    #[test]
    fn joint_endpoint_that_is_not_a_body_resolves_to_its_nearest_ancestor_body() {
        let stage = write_and_compose("mount.usda", MOUNT_FIXTURE);
        let j = read_joint_spec(
            &StageView::new(&stage),
            &SdfPath::new("/Host/Mount/YawJoint").unwrap(),
        )
        .expect("revolute joint reads");
        assert_eq!(
            j.body0_path, "/Host",
            "body0 named a non-body Xform, so it resolves to the host body it hangs under"
        );
        assert_eq!(j.body1_path, "/Host/Mount/Head", "body1 is already a body");
        assert!(
            close(j.local_pos0, DVec3::new(0.0, 1.5, 0.0)),
            "the anchor must be derived in the RESOLVED body's frame: {:?}",
            j.local_pos0
        );
    }

    /// A STATIC host is still a body to mount on. A comms mast does not move, and
    /// its dish still has to yaw against it.
    #[test]
    fn a_mechanism_mounts_on_a_static_host_body() {
        let stage = write_and_compose(
            "mount_static.usda",
            &MOUNT_FIXTURE.replace(
                "def Xform \"Host\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n{\n",
                "def Xform \"Host\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n{\n\
                 bool physics:rigidBodyEnabled = false\n",
            ),
        );
        let j = read_joint_spec(
            &StageView::new(&stage),
            &SdfPath::new("/Host/Mount/YawJoint").unwrap(),
        )
        .expect("a joint mounted on a static body still reads");
        assert_eq!(j.body0_path, "/Host");
    }

    #[test]
    fn joint_disabled_by_physics_joint_enabled_is_not_built() {
        // The spec's own opt-out, and the only way a host can park a mechanism
        // whose joints live inside a component it does not own.
        let stage = write_and_compose(
            "mount_off.usda",
            // NB: the `\` line continuations in `MOUNT_FIXTURE` strip the source
            // indentation, so match the attribute ALONE. A pattern written with
            // leading spaces matches nothing, leaves the fixture unmodified, and
            // the test then fails against a joint that was never disabled.
            &MOUNT_FIXTURE.replace(
                "physics:axis = \"Y\"\n",
                "physics:axis = \"Y\"\nbool physics:jointEnabled = false\n",
            ),
        );
        assert!(
            read_joint_spec(
                &StageView::new(&stage),
                &SdfPath::new("/Host/Mount/YawJoint").unwrap()
            )
            .is_none(),
            "physics:jointEnabled = false must suppress the joint"
        );
    }

    #[test]
    fn wheel_revolute_joints_are_owned_by_the_wheel_projector() {
        let stage = compose_file_to_stage(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenes/tests/drivetrain_parity.usda"),
        )
        .expect("compose drivetrain parity");
        let view = StageView::new(&stage);
        let path = SdfPath::new("/DrivetrainParity/RoverPhysical/Wheel_FL_Hinge")
            .expect("wheel hinge path");
        assert!(
            super::joint_targets_simulated_wheel(&view, &path),
            "the standard body1 relationship and wheel schema must assign this joint to the wheel projector"
        );
        assert!(
            read_joint_spec(&StageView::new(&stage), &path).is_none(),
            "generic Avian projection must not duplicate a wheel joint owned by lunco-usd-sim"
        );
    }

    #[test]
    fn rocker_bogie_hinge_joints_derive_end_to_end() {
        // The HARD retrofit, through the real load path. `rocker_bogie.usda` now omits
        // every anchor. Its FOUR structural hinges flow through `read_joint_spec`
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
            ("HingeL", [-0.99, -0.2, 0.0]), // chassis ↔ rocker (ancestor)
            ("HingeR", [0.99, -0.2, 0.0]),
            ("BogieHingeL", [0.0, -0.2, 0.6]), // rocker ↔ bogie (SIBLING)
            ("BogieHingeR", [0.0, -0.2, 0.6]),
        ] {
            let j = read_joint_spec(
                &StageView::new(&stage),
                &SdfPath::new(&format!("/RockerBogie/{name}")).unwrap(),
            )
            .unwrap_or_else(|| panic!("{name} reads + derives"));
            assert!(
                close(j.local_pos0, DVec3::new(lp0[0], lp0[1], lp0[2])),
                "{name}: derived {:?} != Perseverance-class authored {lp0:?}",
                j.local_pos0
            );
            assert_eq!(j.local_pos1, DVec3::ZERO, "{name}: lp1 = origin");
        }
    }

    #[test]
    fn physical_drivetrain_derives_all_four_wheel_anchors() {
        // `physical_drivetrain.usda` OMITS every localPos0/1. The reader must
        // reproduce, exactly, the four wheel anchors the file used to type twice.
        //
        // The fixture is a FOUR-WHEEL ROVER, not the overlay: the overlay owns the
        // joints and the ROVER owns the mounts (the wheel prim is the axle in both
        // realizations, so where a wheel sits is a property of the vehicle and is
        // authored once, outside the `drivetrain` variantSet). Asking the overlay in
        // isolation where its wheels are would derive four anchors at the origin and
        // call that fine — the fragment cannot answer a question the arc resolves.
        //
        // It is synthetic rather than the shipped scene because a wheel hinge on a
        // real rover carries `PhysxVehicleWheelAPI`, and those joints belong to
        // `lunco-usd-sim`, which builds them from the wheel's own transform and never
        // consults `localPos0` at all. This test is about the DERIVATION, so it feeds
        // the derivation the shape it actually serves: mounts on the wheels, hinges
        // with no anchors, four of them, on one body.
        let mut body = String::from(
            "#usda 1.0\n(\n    defaultPrim = \"Rover\"\n    upAxis = \"Y\"\n    metersPerUnit = 1\n)\n\
             def Xform \"Rover\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n{\n",
        );
        let mounts = [
            ("Wheel_FL", DVec3::new(-1.0, -0.65, -1.225)),
            ("Wheel_FR", DVec3::new(1.0, -0.65, -1.225)),
            ("Wheel_RL", DVec3::new(-1.0, -0.65, 1.225)),
            ("Wheel_RR", DVec3::new(1.0, -0.65, 1.225)),
        ];
        for (w, p) in mounts {
            body += &format!(
                "    def Cylinder \"{w}\" ( prepend apiSchemas = [\"PhysicsRigidBodyAPI\"] )\n    {{\n\
                 \x20       double3 xformOp:translate = ({}, {}, {})\n\
                 \x20       uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }}\n\
                 \x20   def PhysicsRevoluteJoint \"{w}_Hinge\"\n    {{\n\
                 \x20       rel physics:body0 = </Rover>\n\
                 \x20       rel physics:body1 = </Rover/{w}>\n\
                 \x20       uniform token physics:axis = \"X\"\n    }}\n",
                p.x, p.y, p.z
            );
        }
        body += "}\n";
        let stage = write_and_compose("four_wheel_derive.usda", &body);

        for (w, lp0) in mounts {
            let name = format!("{w}_Hinge");
            let j = read_joint_spec(
                &StageView::new(&stage),
                &SdfPath::new(&format!("/Rover/{name}")).unwrap(),
            )
            .unwrap_or_else(|| panic!("{name} reads"));
            assert!(
                close(j.local_pos0, lp0),
                "{name}: anchor derived from the wheel translate: {:?}",
                j.local_pos0
            );
            assert_eq!(j.local_pos1, DVec3::ZERO, "{name}: lp1 = origin");
        }
    }
}

#[cfg(test)]
mod collider_ownership_tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};
    use std::collections::HashMap;

    #[test]
    fn malformed_compound_child_transform_is_rejected_not_identity() {
        let source = r#"#usda 1.0
def Xform "Root" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] )
{
    def Cube "Body" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        double size = 2
        uniform token[] xformOpOrder = ["xformOp:unsupported"]
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("bad.usda", source))
            .expect("build stage");
        let root = SdfPath::new("/Root").unwrap();
        let error = collect_child_colliders_from_usd(&stage.view(), &root)
            .expect_err("malformed authored transform must reject compound discovery");
        assert_eq!(error.prim, "/Root/Body");
    }

    #[test]
    fn malformed_collision_enabled_does_not_enable_compound_geometry() {
        let source = r#"#usda 1.0
def Xform "Root" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] )
{
    def Cube "Body" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        double size = 2
    float physics:collisionEnabled = 1.0
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("bad.usda", source))
            .expect("build stage");
        let root = SdfPath::new("/Root").unwrap();
        assert!(
            collect_child_colliders_from_usd(&stage.view(), &root)
                .expect("the malformed flag is refused without corrupting traversal")
                .is_empty(),
            "an invalid authored collisionEnabled must not become the schema default true"
        );
    }

    #[test]
    fn malformed_physics_scene_gravity_does_not_fall_back_to_earth() {
        let source = r#"#usda 1.0
def PhysicsScene "Scene"
{
    string physics:gravityMagnitude = "not-a-number"
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("bad.usda", source))
            .expect("build stage");
        let scene = SdfPath::new("/Scene").unwrap();
        let error = read_physics_scene_gravity(&stage.view(), &scene)
            .expect_err("an authored gravity value with the wrong type must be rejected");
        assert!(error.contains("unsupported authored value type"));
    }

    #[test]
    fn physics_scene_gravity_preserves_usd_sentinel_defaults() {
        let source = r#"#usda 1.0
def PhysicsScene "Scene"
{
    float physics:gravityMagnitude = -1.0
    vector3f physics:gravityDirection = (0, 0, 0)
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("sentinel.usda", source))
            .expect("build stage");
        let scene = SdfPath::new("/Scene").unwrap();
        let values = read_physics_scene_gravity(&stage.view(), &scene).expect("USD sentinels");
        assert_eq!(values.0, lunco_environment::EARTH_SURFACE_GRAVITY);
        assert_eq!(values.1, DVec3::NEG_Y);
    }

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

    def Xform "XformBody" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] )
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
    fn extract(view: &lunco_usd_bevy::StageView<'_>, path: &str) -> (bool, Option<RigidBody>) {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let sdf = SdfPath::new(path).unwrap();
        {
            let mut commands = world.commands();
            extract_avian_prim(
                view,
                entity,
                &sdf,
                &CollisionGroupTable::default(),
                &mut commands,
                None,
                None,
            );
        }
        world.flush();
        (
            world.get::<Collider>(entity).is_some(),
            world.get::<RigidBody>(entity).copied(),
        )
    }

    /// A leg with a footpad: the pad is a CHILD of the leg, its own body, and
    /// jointed to it — how a foot mounts on a leg and a wheel on a chassis.
    const NESTED_BODY: &str = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Rig"
{
    def Xform "Leg" (prepend apiSchemas = ["PhysicsRigidBodyAPI"])
    {
        def Cylinder "Strut" (prepend apiSchemas = ["PhysicsCollisionAPI"])
        {
            uniform token axis = "Y"
            double radius = 0.075
            double height = 7.05
        }
        def Cylinder "Pad" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"])
        {
            uniform token axis = "Y"
            double radius = 0.4
            double height = 0.3
            double3 xformOp:translate = (0, -3.675, 0)
            uniform token[] xformOpOrder = ["xformOp:translate"]
        }
        def PhysicsSphericalJoint "Gimbal"
        {
            rel physics:body0 = </Rig/Leg>
            rel physics:body1 = </Rig/Leg/Pad>
        }
    }
}
"#;

    /// OWNERSHIP STOPS AT A BODY BOUNDARY, and this is the direction that was
    /// missing. A child that is its own rigid body is a neighbour, not geometry:
    /// folding its collider into the parent's compound gives one shape two owners
    /// — the compound holds it rigidly in the parent's frame while its joint tries
    /// to move it. The pair fight every step until a body leaves the world, which
    /// is what a landing gear did at 10^15 m once its pad became a child.
    ///
    /// Wheels survived only because `physxVehicleWheel:radius` skipped them by
    /// name. This is the general rule that carve-out was standing in for.
    #[test]
    fn a_nested_body_is_not_a_piece_of_its_parents_compound_shape() {
        let recipe = StageRecipe::from_source("t.usda", NESTED_BODY);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        let leg = SdfPath::new("/Rig/Leg").unwrap();
        let pieces = collect_child_colliders_from_usd(&view, &leg).expect("valid transforms");
        assert_eq!(
            pieces.len(),
            1,
            "the leg's compound is its strut alone — the pad is its own body, not \
             the leg's geometry"
        );
    }

    /// A body describing its shape twice — a detailed `render` mesh and a cheap
    /// `proxy` box — collides the PROXY, and only the proxy. Folding both in
    /// would collide the vehicle at two levels of detail at once, and the
    /// expensive one would win every contact.
    ///
    /// `guide` geometry is never physical at all: it is annotation, whatever
    /// shape it happens to be.
    const PURPOSES: &str = r#"#usda 1.0
def Xform "Rig"
{
    def Xform "Hull" (prepend apiSchemas = ["PhysicsRigidBodyAPI"])
    {
        def Cube "Shell" (prepend apiSchemas = ["PhysicsCollisionAPI"])
        {
            uniform token purpose = "render"
            double size = 4.0
        }
        def Cube "Bounds" (prepend apiSchemas = ["PhysicsCollisionAPI"])
        {
            uniform token purpose = "proxy"
            double size = 2.0
        }
        def Cube "AxisMarker" (prepend apiSchemas = ["PhysicsCollisionAPI"])
        {
            uniform token purpose = "guide"
            double size = 1.0
        }
    }
}
"#;

    #[test]
    fn a_body_with_a_proxy_collides_the_proxy_and_never_the_guide() {
        let recipe = StageRecipe::from_source("t.usda", PURPOSES);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        let hull = SdfPath::new("/Rig/Hull").unwrap();
        let pieces = collect_child_colliders_from_usd(&view, &hull).expect("valid transforms");
        assert_eq!(
            pieces.len(),
            1,
            "expected exactly the proxy — got {} pieces, so the render mesh or the \
             guide marker is being collided too",
            pieces.len()
        );
    }

    /// `purpose` is a uniform token and INHERITS, so authoring it once on a scope
    /// covers everything inside it — which is how a proxy is normally authored.
    #[test]
    fn purpose_is_inherited_from_an_ancestor() {
        let recipe = StageRecipe::from_source("t.usda", PURPOSES);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        let marker = SdfPath::new("/Rig/Hull/AxisMarker").unwrap();
        assert_eq!(effective_purpose(&view, &marker), Purpose::Guide);
        // Nothing authored anywhere up the chain: the ordinary case, and the one
        // every asset in this repo is in today.
        let hull = SdfPath::new("/Rig/Hull").unwrap();
        assert_eq!(effective_purpose(&view, &hull), Purpose::Default);
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
        assert!(
            has_collider,
            "a ground plane under an Xform must get a collider"
        );
        assert_eq!(body, Some(RigidBody::Static), "and it must be static");
    }

    /// The other half of the rule: a collider UNDER a rigid body is a piece of that
    /// body's compound shape, so it gets no collider and no body of its own.
    #[test]
    fn collider_under_rigid_body_ancestor_stays_a_compound_piece() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let (has_collider, body) = extract(&cs.view(), "/Mission/CompoundLander/Hull");
        assert!(
            !has_collider,
            "a collider child must not carry its own collider"
        );
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
        assert!(
            collect_child_colliders_from_usd(&view, &lander)
                .expect("valid transforms")
                .is_empty()
        );
        assert!(
            build_collider_from_usd(&view, &lander)
                .expect("valid transform")
                .is_some()
        );
        let (has_collider, _) = extract(&view, "/Mission/BareLander");
        assert!(
            has_collider,
            "a bare rigid-body root must collide via its own shape"
        );
    }

    /// A body whose own prim carries no geometry (a plain `Xform` with
    /// `PhysicsRigidBodyAPI`) still owns its collider children — they are pieces of
    /// its compound shape, not static geometry.
    #[test]
    fn xform_rigid_body_ancestor_owns_its_colliders() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        assert!(has_rigid_body_ancestor(
            &view,
            &SdfPath::new("/Mission/XformBody/Shell").unwrap()
        ));
        let (has_collider, body) = extract(&view, "/Mission/XformBody/Shell");
        assert!(
            !has_collider,
            "a body's collider child must stay a compound piece"
        );
        assert_eq!(body, None);
    }

    /// A reference can contribute the body root's own collision shape as well as
    /// descendant shapes. Both are part of that one USD rigid body; dropping the
    /// root when a child exists makes the composed body smaller than its authored
    /// collision contract.
    #[test]
    fn composed_reference_keeps_body_and_descendant_collision_shapes() {
        let root_id = "scene.usda".to_string();
        let child_id = "child.usda".to_string();
        let scene = r#"#usda 1.0
(
    defaultPrim = "Scene"
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Scene"
{
    def "Assembly" (
        prepend references = @child.usda@</Part>
    )
    {
    }
}
"#;
        let child = r#"#usda 1.0
(
    defaultPrim = "Part"
    upAxis = "Y"
    metersPerUnit = 1
)
def Cube "Part" (
    prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"]
)
{
    double size = 2.0

    def Cube "EndCap" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {
        double size = 1.0
        double3 xformOp:translate = (0, 2, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;
        let recipe = StageRecipe {
            root_id: root_id.clone(),
            bytes: HashMap::from([
                (root_id.clone(), scene.as_bytes().to_vec()),
                (child_id, child.as_bytes().to_vec()),
            ]),
        };

        let live = CanonicalStage::from_recipe(&recipe).expect("compose referenced body");
        let assembly = SdfPath::new("/Scene/Assembly").unwrap();
        let live_view = live.view();
        assert!(live_view.has_prim(&SdfPath::new("/Scene/Assembly/EndCap").unwrap()));
        let live_shapes = collect_child_colliders_from_usd(&live_view, &assembly)
            .expect("live composed reference has valid collision transforms");
        assert_eq!(
            live_shapes.len(),
            2,
            "live composition must keep root and child shapes"
        );
        assert_eq!(live_shapes[0].0.0, DVec3::ZERO);
        assert_eq!(live_shapes[1].0.0, DVec3::new(0.0, 2.0, 0.0));

        let child_recipe = StageRecipe {
            root_id: "child.usda".to_string(),
            bytes: HashMap::from([("child.usda".to_string(), child.as_bytes().to_vec())]),
        };
        let prepared = UsdStageAsset::from_recipe(child_recipe).expect("prepare referenced body");
        let instance = prepared
            .projection_plan
            .for_instance("/Scene/PreparedAssembly")
            .expect("remap referenced body plan");
        let instance_root = SdfPath::new("/Scene/PreparedAssembly").unwrap();
        assert!(instance.has_prim(&SdfPath::new("/Scene/PreparedAssembly/EndCap").unwrap()));
        let prepared_shapes = collect_child_colliders_from_usd(&instance, &instance_root)
            .expect("prepared reference has valid collision transforms");
        assert_eq!(
            prepared_shapes.len(),
            2,
            "prepared composition must keep root and child shapes"
        );
        assert_eq!(prepared_shapes[0].0.0, DVec3::ZERO);
        assert_eq!(prepared_shapes[1].0.0, DVec3::new(0.0, 2.0, 0.0));
    }

    #[test]
    fn rigid_body_ancestry_is_walked_transitively() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let view = cs.view();
        assert!(!has_rigid_body_ancestor(
            &view,
            &SdfPath::new("/Mission/Ground").unwrap()
        ));
        assert!(has_rigid_body_ancestor(
            &view,
            &SdfPath::new("/Mission/CompoundLander/Hull").unwrap()
        ));
        assert!(!has_rigid_body_ancestor(
            &view,
            &SdfPath::new("/Mission/CompoundLander").unwrap()
        ));
    }
}
