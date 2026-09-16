//! # LunCoSim USD → Avian3D Physics Mapping
//!
//! Maps USD physics attributes to Avian3D components. This is the **second** plugin in
//! the USD processing pipeline, running after `UsdVisualPlugin` and alongside `UsdSimPlugin`.
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
use lunco_spatial::coords::GridPos;
use lunco_usd_avian_core::report_physics_runtime_fault;
use lunco_usd_avian_filters::collision_groups::{CollisionGroupTable, CollisionGroupTables};
use lunco_usd_avian_filters::filtered_pairs as collision_filters;
use lunco_usd_bevy_core::{
    effective_purpose, world_transform, Purpose, TransformReadError, UsdInstanceProjection,
    UsdInstanceRoot, UsdRead, UsdStageAsset,
};
use lunco_usd_bevy_scene::{
    instance_key, is_preview_only, UsdAnimated, UsdPreviewOnly, UsdPrimPath, UsdSceneProjected,
    UsdSceneRoot,
};
use openusd::sdf::Path as SdfPath;
// UsdPhysics attribute + API-schema names as CONSTANTS, from openusd's own schema
// module. Hand-written `"physics:…"` string literals are how `physics:friction`
// (an attribute UsdPhysics does not define) got invented and lived here for
// months: a typo in a `&str` compiles.
use lunco_usd_avian_contracts::{
    AuthoredInitialVelocity, JointDrive, PendingJointAdmission, PendingUsdJoint, ScenePhysicsOwned,
    ShouldBeDynamic,
};
use lunco_usd_avian_reader::{
    collider::{
        build_collider_from_usd, collect_child_colliders_from_usd, ColliderProjectionError,
    },
    joint::{
        has_rigid_body_ancestor, joint_targets_simulated_wheel, nearest_body_path, read_joint_spec,
    },
    read_authored_bool_or_default, read_authored_quat, read_authored_real, read_authored_vec3,
};
use openusd::schemas::physics::tokens as ptok;

mod material;
use material::read_physics_material;

/// Runtime evidence for one joint-shaped entity, consumed by the generic Rhai
/// USD lint. The fact is deliberately read-only; it does not create or repair
/// topology. `linked` is the projection's authoritative endpoint contract,
/// while `pending`, `native`, and `graph` show which lifecycle stage currently
/// owns the entity.
#[derive(Clone, Debug)]
pub struct RuntimeJointFact {
    /// Live entity bits, stable for the current process/session.
    pub entity_bits: u64,
    /// Authored USD path, when the entity came from a composed stage.
    pub path: Option<String>,
    /// Whether the generic `PhysicsJointLink` endpoint contract is present.
    pub linked: bool,
    /// Whether the typed joint is waiting for native admission.
    pub pending: bool,
    /// Whether Avian has admitted a native joint component.
    pub native: bool,
    /// Whether the edge is present in Avian's `JointGraph`.
    pub graph: bool,
    /// Whether the Avian joint graph resource was installed for this lint pass.
    pub graph_available: bool,
    /// Whether a solver-safe detach was requested.
    pub detach_requested: bool,
}

/// Collect runtime joint topology evidence for the loaded stage.
///
/// Authored entities are selected by their `UsdPrimPath` stage handle. Runtime
/// synthesized joints are selected by `ScenePhysicsOwned`, which is the explicit
/// ownership marker used by scene teardown. No path/name heuristic is used.
pub fn runtime_joint_facts(
    world: &World,
    stage_id: AssetId<UsdStageAsset>,
) -> Vec<RuntimeJointFact> {
    let graph_entities: Option<EntityHashSet> = world.get_resource::<JointGraph>().map(|graph| {
        graph
            .graph()
            .all_edge_weights()
            .map(|edge| edge.entity)
            .collect()
    });
    let graph_available = graph_entities.is_some();

    let mut facts = Vec::new();
    for entity in world.iter_entities() {
        let path = entity.get::<UsdPrimPath>().map(|prim| prim.path.clone());
        let stage_owned = entity
            .get::<UsdPrimPath>()
            .is_some_and(|prim| prim.stage_handle.id() == stage_id);
        let synthesized = entity.get::<ScenePhysicsOwned>().is_some() && path.is_none();
        if !stage_owned && !synthesized {
            continue;
        }

        let linked = entity.get::<lunco_physics::PhysicsJointLink>().is_some();
        let pending = entity.get::<lunco_physics::PhysicsJointPending>().is_some();
        let native = entity
            .get::<avian3d::dynamics::solver::joint_graph::JointComponentId>()
            .is_some_and(|id| id.id().is_some());
        let graph = graph_entities
            .as_ref()
            .is_some_and(|entities| entities.contains(&entity.id()));
        let detach_requested = entity
            .get::<lunco_physics::PhysicsJointDetachRequested>()
            .is_some();
        if !(linked || pending || native || graph || detach_requested) {
            continue;
        }
        facts.push(RuntimeJointFact {
            entity_bits: entity.id().to_bits(),
            path,
            linked,
            pending,
            native,
            graph,
            graph_available,
            detach_requested,
        });
    }
    facts.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.entity_bits.cmp(&right.entity_bits))
    });
    facts
}

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

/// Bevy plugin for USD physics mapping.
///
/// Adds an observer for USD prim spawning and a deferred processing system that maps
/// USD physics attributes to Avian3D components. The deferred system runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure assets are loaded.
pub struct UsdAvianPlugin;

/// Retire a set of joint-graph edges before removing their ECS components.
///
/// Avian's component hooks remove edges automatically, but a recursive entity
/// despawn can remove `JointDisabled`, the native joint, and the pair-filter
/// marker in an order that causes the same island entry to be unlinked twice.
/// The scene teardown and live detach paths therefore share this one graph
/// transaction.  The native removal hooks then see an already-retired edge and
/// become harmless no-ops.
fn retire_joint_graph_edges(world: &mut World, entities: &[Entity]) {
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
        let Ok((mut islands, mut joint_graph, contact_graph, mut body_islands)) =
            graph_state.get_mut(world)
        else {
            error!(
                "joint graph retirement blocked: Avian graph resources have conflicting access; keeping the topology intact"
            );
            return;
        };

        for &entity in entities {
            let Some(edge) = joint_graph.get(entity).cloned() else {
                continue;
            };
            let island_id = edge.island.island_id();
            if island_id != avian3d::dynamics::solver::islands::IslandId::PLACEHOLDER {
                let Some(island) = islands.get(island_id) else {
                    warn!(
                        "joint graph edge {:?} references missing island {:?}; edge is already outside the island list",
                        entity, island_id
                    );
                    joint_graph.remove_joint(entity);
                    continue;
                };
                if island.joint_count() > 0 {
                    let _ = islands.remove_joint(
                        edge.id,
                        &mut body_islands,
                        &contact_graph,
                        &mut joint_graph,
                    );
                } else {
                    warn!(
                        "joint graph edge {:?} has no island joint count; treating it as already retired",
                        entity
                    );
                }
            }
            joint_graph.remove_joint(entity);
        }
    }
    graph_state.apply(world);
}

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
    retire_joint_graph_edges(world, &graph_joints);

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

/// Complete live joint detach requests at the physics bridge boundary.
///
/// A scene command only writes [`lunco_physics::PhysicsJointDetachRequested`].
/// This exclusive system is the sole owner of the transition from a live
/// constraint to a disposable entity: it retires the graph edge first, then
/// removes the native/pending components, releases the transient collision
/// filter, and finally despawns the joint.  Keeping the whole sequence here
/// makes component-removal order explicit and prevents a second command path
/// from touching Avian's island bookkeeping.
fn retire_requested_joints(world: &mut World) {
    let requested: Vec<(Entity, Option<ComponentId>)> = {
        let mut query = world.query_filtered::<(
            Entity,
            Option<&avian3d::dynamics::solver::joint_graph::JointComponentId>,
        ), With<lunco_physics::PhysicsJointDetachRequested>>();
        query
            .iter(world)
            .map(|(entity, id)| (entity, id.and_then(|id| id.id())))
            .collect()
    };
    if requested.is_empty() {
        return;
    }

    if world.get_resource::<PhysicsIslands>().is_none()
        || world.get_resource::<JointGraph>().is_none()
        || world.get_resource::<ContactGraph>().is_none()
    {
        error!(
            "DETACH_JOINT blocked: Avian physics resources are not installed; add PhysicsPlugins before JointAttachPlugin"
        );
        return;
    }

    let entities: Vec<Entity> = requested.iter().map(|(entity, _)| *entity).collect();
    retire_joint_graph_edges(world, &entities);

    for (entity, component_id) in requested {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            continue;
        };

        // The native component hook queues removal of JointComponentId. Remove
        // the native component first, then clear the id and disabled marker so
        // no observer can re-admit the retired edge.
        if let Some(component_id) = component_id {
            entity_mut.remove_by_id(component_id);
        }
        entity_mut
            .remove::<avian3d::dynamics::solver::joint_graph::JointComponentId>()
            .remove::<JointDisabled>()
            .remove::<PendingJoint<RevoluteJoint>>()
            .remove::<PendingJoint<PrismaticJoint>>()
            .remove::<PendingJoint<FixedJoint>>()
            .remove::<PendingJoint<SphericalJoint>>()
            .remove::<PendingJoint<DistanceJoint>>()
            .remove::<PendingJointAdmission>()
            .remove::<lunco_physics::PhysicsJointPending>()
            .remove::<lunco_physics::PhysicsJointDetachRequested>()
            .remove::<collision_filters::JointCollisionPair>();

        entity_mut.despawn();
        info!(
            "DETACH_JOINT: retired solver edge and despawned {:?}",
            entity
        );
    }
}

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        // Installs joints parked by `attach_joint` — the USD path attaches
        // authored joints, so this app must be able to land them.
        app.add_plugins(JointAttachPlugin);
        app.add_systems(lunco_core::SceneTeardown, prepare_scene_physics_teardown);
        // `on_add_usd_prim`: eager observer for joint pending-state.
        // `process_usd_avian_prims`: observer on UsdSceneProjected — fires
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
        app.init_resource::<CollisionGroupTables>();
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut commands: Commands, mut groups: ResMut<CollisionGroupTables>| {
                commands.remove_resource::<lunco_environment::PhysicsSceneGravity>();
                // The groups belong to the scene being replaced. Carried over,
                // they would put the next scene's colliders on layers nothing in
                // it defines.
                groups.clear();
            },
        );

        app.register_type::<ShouldBeDynamic>()
            .register_type::<collision_filters::SharedTireContact>()
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
                        .after(lunco_usd_avian_core::PhysicsBridgeSystems::Read)
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
                collision_filters::resolve_filtered_pairs
                    .run_if(any_with_component::<collision_filters::PendingFilteredPairs>)
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
                    collision_filters::enable_shared_tire_contact_hooks,
                    collision_filters::enable_static_friction_contact_hooks,
                    collision_filters::synchronize_collision_hook_flags
                        .after(collision_filters::enable_static_friction_contact_hooks),
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
/// The USD animation sampler writes its `Transform`
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

/// Force (N) / torque (N·m) saturation a USD-driven joint motor gets when its
/// `physics:maxForce` is left unauthored — generous enough to hold the target
/// against gravity, matching `lunco_cosim::joint`'s wire-driven default.
const JOINT_DRIVE_MAX_FORCE_DEFAULT: f64 = 1.0e8;

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(), ColliderProjectionError> {
    if let Some(collider) = build_collider_from_usd(reader, sdf_path)? {
        if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: "collider local bounds are not finite, ordered, or f32-representable"
                    .to_owned(),
            });
        }
        commands.entity(entity).try_insert(collider);
    }
    Ok(())
}

fn log_collider_projection_error(sdf_path: &SdfPath, error: &ColliderProjectionError) {
    error!(
        "[usd-avian] {sdf_path} has invalid collider authoring; refusing collider projection: {error}"
    );
}

fn report_collider_projection_error(
    faults: Option<&mut lunco_core::RuntimeFaults>,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
    entity: Entity,
    sdf_path: &SdfPath,
    error: &ColliderProjectionError,
) {
    report_physics_runtime_fault(
        faults,
        holds,
        entity,
        sdf_path.to_string(),
        "usd-avian-collider-invalid",
        error.to_string(),
    );
}

fn reject_collider_projection(
    commands: &mut Commands,
    entity: Entity,
    sdf_path: &SdfPath,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
    error: ColliderProjectionError,
) {
    log_collider_projection_error(sdf_path, &error);
    report_collider_projection_error(faults, holds, entity, sdf_path, &error);
    commands.entity(entity).try_insert(UsdAvianProcessed);
}

/// Resolve a USD joint relationship target to the rigid-body prim that owns the
/// endpoint. The relationship may name a mechanism child inside a referenced
/// component; the joint contract attaches to that child's nearest body ancestor.
/// Keep this resolution in the Avian USD reader so topology consumers cannot
/// accidentally compare an unresolved authored path with a resolved ECS path.
pub fn resolve_joint_body_path(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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

/// Select the collider owner from the authored terrain mode. DEM/layered
/// terrain is built by `lunco-terrain-surface` from its retained height oracle;
/// every other terrain prim owns its standard USD geometry collider here. Mesh
/// terrain may still defer that same collider until its mesh asset is ready.
fn terrain_uses_authored_collider(asset_mode: Option<&str>) -> bool {
    !matches!(asset_mode, Some("dem") | Some("layered"))
}

#[cfg(test)]
mod terrain_collider_owner_tests {
    use super::terrain_uses_authored_collider;

    #[test]
    fn only_dem_modes_delegate_collider_ownership_to_the_surface_stream() {
        assert!(terrain_uses_authored_collider(Some("mesh")));
        assert!(!terrain_uses_authored_collider(Some("dem")));
        assert!(!terrain_uses_authored_collider(Some("layered")));
        assert!(terrain_uses_authored_collider(None));
    }
}

/// Builds the static collider for a mesh-backed terrain once its `Mesh3d`
/// asset is available. Prefers a [`heightfield`](heightfield_from_mesh) when
/// the mesh is a regular DEM grid; otherwise falls back to a general trimesh.
fn build_terrain_mesh_colliders(
    q: Query<(Entity, &Mesh3d, Option<&UsdPrimPath>), With<PendingTerrainCollider>>,
    meshes: Res<Assets<Mesh>>,
    mut commands: Commands,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    for (entity, mesh3d, prim_path) in &q {
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
                if !lunco_physics::avian_backend_collider_shape_is_valid(&c) {
                    let subject = prim_path
                        .map(|path| path.path.clone())
                        .unwrap_or_else(|| format!("entity {entity:?}"));
                    report_physics_runtime_fault(
                        faults.as_deref_mut(),
                        holds.as_deref_mut(),
                        entity,
                        subject,
                        "usd-avian-collider-invalid",
                        "terrain mesh collider bounds are not finite, ordered, or f32-representable"
                            .to_owned(),
                    );
                    commands.entity(entity).remove::<PendingTerrainCollider>();
                    continue;
                }
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
/// translates the prim (signalled by inserting `UsdSceneProjected`). CPU visual
/// meshes may still be streaming; physics reads the worker-produced composed
/// projection plan and does not depend on `Mesh3d` or a live `Stage`.
/// By that point the plan is committed and the same reader contract used by
/// visual and simulation projection is available for physics components.
fn process_usd_avian_prims(
    trigger: On<Add, UsdSceneProjected>,
    query: Query<(&UsdPrimPath, Option<&UsdInstanceProjection>), Without<UsdAvianProcessed>>,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    mount_state: Option<Res<lunco_core::SceneMountState>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_core::canonical::CanonicalStages>,
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
    // PhysicsRigidBodyAPI so Avian cannot admit a duplicate body or a duplicate
    // joint/collider graph into the simulation.
    if is_preview_only(entity, &q_child_of, &q_preview_only) {
        commands.entity(entity).try_insert(UsdAvianProcessed);
        return;
    }
    if let Some(mount_state) = mount_state {
        let stale_mount = match lunco_usd_bevy_scene::scene_root_ancestor(
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
            // `UsdSceneProjected` can be applied from a command buffer that was
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(f64, DVec3), &'static str> {
    let convention = lunco_usd_bevy_core::stage_convention(reader)
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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
        report_physics_runtime_fault(
            faults,
            holds,
            entity,
            sdf_path.to_string(),
            "usd-physics-joint-invalid",
            detail.to_owned(),
        );
    }
    true
}

fn extract_avian_prim(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    entity: Entity,
    sdf_path: &SdfPath,
    groups: &CollisionGroupTable,
    commands: &mut Commands,
    mut faults: Option<&mut lunco_core::RuntimeFaults>,
    mut holds: Option<&mut lunco_physics::PhysicsHolds>,
) {
    // Joint projection is owned by the same composed-prim boundary as every
    // other Avian projection. The legacy `Add<UsdPrimPath>` observer can run
    // before its stage is available; relying on it alone lets terrain consume
    // a support request before the joint topology exists. A joint is a
    // constraint declaration, not a body/collider, so finish this prim here
    // and leave native admission to the shared pending-joint path.
    if project_pending_joint(
        reader,
        entity,
        sdf_path,
        commands,
        faults.as_deref_mut(),
        holds.as_deref_mut(),
    ) {
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
    if let Some(pending) = collision_filters::read_filtered_pairs(reader, sdf_path) {
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
    // ── TERRAIN ── static collider; the terrain projection owns its marker.
    if has_terrain_api {
        if apply_physics_material(commands, entity, reader, sdf_path).is_err() {
            error!(
                "[usd-avian] {sdf_path} has malformed physics-material values — refusing terrain projection"
            );
            commands.entity(entity).try_insert(UsdAvianProcessed);
            return;
        }
        commands
            .entity(entity)
            .try_insert((RigidBody::Static, lunco_core::Mobility::Static));
        // Terrain is a static body, but it is still a USD physics surface.
        // Apply the authored material before its collider is admitted so the
        // solver combines the ground's friction/restitution with the touching
        // body's material exactly as it does for a dynamic body.  Keeping this
        // on the classification branch avoids a scene-specific ground override.
        //
        // `dem`/`layered` terrain has a native collider built from the retained
        // `SurfaceOracle` by `lunco-terrain-surface`. Every other terrain prim
        // is ordinary authored USD collision geometry and must be projected
        // here. Mesh terrain may wait for its async mesh asset; a flat site,
        // ramp, or authored obstacle is available directly from the composed
        // USD stage and must not be admitted without its collider.
        if terrain_uses_authored_collider(reader.text(sdf_path, "lunco:assetMode").as_deref()) {
            match build_collider_from_usd(reader, sdf_path) {
                Ok(Some(collider)) => {
                    if lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
                        commands.entity(entity).try_insert(collider);
                    } else {
                        reject_collider_projection(
                            commands,
                            entity,
                            sdf_path,
                            faults.as_deref_mut(),
                            holds.as_deref_mut(),
                            ColliderProjectionError::Backend {
                                prim: sdf_path.to_string(),
                                detail: "terrain collider bounds are not finite, ordered, or f32-representable"
                                    .to_owned(),
                            },
                        );
                        return;
                    }
                }
                Ok(None) => {
                    commands.entity(entity).try_insert(PendingTerrainCollider);
                }
                Err(error) => {
                    reject_collider_projection(
                        commands,
                        entity,
                        sdf_path,
                        faults.as_deref_mut(),
                        holds.as_deref_mut(),
                        error,
                    );
                    return;
                }
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
            reject_collider_projection(
                commands,
                entity,
                sdf_path,
                faults.as_deref_mut(),
                holds.as_deref_mut(),
                error,
            );
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
                reject_collider_projection(commands, entity, sdf_path, faults, holds, error);
                return;
            }
        };
        if !compound_shapes.is_empty() {
            let collider = Collider::compound(compound_shapes);
            if lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
                commands.entity(entity).try_insert(collider);
            } else {
                reject_collider_projection(
                    commands,
                    entity,
                    sdf_path,
                    faults,
                    holds,
                    ColliderProjectionError::Backend {
                        prim: sdf_path.to_string(),
                        detail:
                            "compound collider bounds are not finite, ordered, or f32-representable"
                                .to_owned(),
                    },
                );
                return;
            }
        } else {
            if let Err(error) = add_collider_from_usd(commands, entity, reader, sdf_path) {
                reject_collider_projection(commands, entity, sdf_path, faults, holds, error);
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
            commands.entity(entity).try_insert((
                ShouldBeDynamic,
                lunco_core::PhysicsStatePending,
                lunco_physics::PhysicsInitializationPending,
                lunco_physics::PhysicsInitializationPolicy::default(),
                lunco_physics::PhysicsInitializationSubject(sdf_path.to_string()),
            ));
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
                reject_collider_projection(commands, entity, sdf_path, faults, holds, error);
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

/// Rotate a vector authored in a prim's local frame into the composed world
/// frame. USD physics velocity attributes are local-frame vectors, while
/// Avian's runtime velocity components are world-frame. Keep that rotation in
/// one helper so linear and angular initial state share the same convention.
fn local_vector_to_world(
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
    path: &SdfPath,
    local: DVec3,
) -> Result<DVec3, TransformReadError> {
    Ok(world_transform(reader, path)?.rotation.as_dquat() * local)
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
    canonical: NonSend<lunco_usd_bevy_core::canonical::CanonicalStages>,
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
    // `UsdSceneProjected` Avian guard, so it must enforce the same ownership
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
    q_shadow: Query<&lunco_usd_avian_core::BridgeShadow>,
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
///    the pair is entered into [`collision_filters::filter_pair`] the moment it is
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
    // See `collision_filters::filter_pair`.
    collision_filters::filter_pair(commands, joint_entity, body0, body1);
    commands.entity(joint_entity).try_insert((
        collision_filters::JointCollisionPair { body0, body1 },
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
        app.add_observer(collision_filters::on_remove_joint_collision_pair);
        // Live detach is owned by the same plugin as joint admission.  The
        // lifecycle marker is consumed before admission, and the graph edge is
        // retired before any native component removal can reach Avian hooks.
        app.add_systems(Update, retire_requested_joints.before(JointAdmission));
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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
    let conv = lunco_usd_bevy_core::stage_convention(reader).map_err(|_| ())?;
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
    reader: &dyn lunco_usd_bevy_core::read::UsdReadObject,
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
                collision_filters::enable_collision_hook(
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

#[cfg(test)]
mod collider_parity_tests {
    //! The collider read path, driven off the
    //! live `StageView` over the canonical stage. Exercises the geometry read
    //! (the highest-risk physics read), including the mesh-approximation selector.

    use super::build_collider_from_usd;
    use bevy::math::DVec3;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    fn stage_from_source(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("test.usda", source))
            .expect("compose in-memory USDA fixture")
    }

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
        let stage = stage_from_source(MESH_FIXTURE);
        let view = stage.view();

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
        let stage = stage_from_source(SOURCE);
        let view = stage.view();

        let scaled = build_collider_from_usd(&view, &SdfPath::new("/Scaled").unwrap())
            .expect("named scale is a valid composed transform")
            .expect("scaled cube collider");
        assert_eq!(scaled.scale(), DVec3::new(2.0, 3.0, 4.0));

        let error = build_collider_from_usd(&view, &SdfPath::new("/Malformed").unwrap())
            .expect_err("a transform that names a missing scale op must be rejected");
        assert!(error.to_string().contains("/Malformed"));
    }
}

#[cfg(test)]
mod extract_parity_tests {
    //! End-to-end physics extraction off the live `StageView`: the REAL
    //! `extract_avian_prim` on a rover chassis with a child collider at an
    //! authored transform, exercising the whole read layer (schema detect →
    //! compound collider → local child transforms → mass props).

    use super::{extract_avian_prim, read_physics_material};
    use avian3d::prelude::*;
    use bevy::ecs::world::CommandQueue;
    use bevy::prelude::*;
    use lunco_usd_avian_filters::collision_groups::CollisionGroupTable;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_bevy_core::StageView;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    fn stage_from_source(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("test.usda", source))
            .expect("compose in-memory USDA fixture")
    }

    // A rover chassis (RigidBodyAPI, mass 500) with a child Cube collider
    // (CollisionAPI) offset by an authored xformOp:translate — the compound path.
    const FIXTURE: &str = "#usda 1.0\n\ndef Xform \"Rover\" (\n    prepend apiSchemas = [\"PhysicsRigidBodyAPI\"]\n)\n{\n    double physics:mass = 500\n    def Cube \"Body\" (\n        prepend apiSchemas = [\"PhysicsCollisionAPI\"]\n    )\n    {\n        double size = 2\n        double3 xformOp:translate = (0, 1, 0)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";

    const FLAT_TERRAIN_FIXTURE: &str = "#usda 1.0\n\ndef Plane \"Ground\" (\n    prepend apiSchemas = [\"PhysicsCollisionAPI\", \"LunCoTerrainAPI\"]\n)\n{\n    double width = 100\n    double length = 100\n    token axis = \"Y\"\n    bool physics:collisionEnabled = true\n}\n";

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
        let stage = stage_from_source(FIXTURE);
        let view = stage.view();
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
    fn authored_flat_terrain_keeps_its_standard_support_collider() {
        let stage = stage_from_source(FLAT_TERRAIN_FIXTURE);
        let view = stage.view();
        let live = run_extract(&view, &SdfPath::new("/Ground").unwrap());

        assert_eq!(live.0, Some(RigidBody::Static));
        assert!(
            live.1.is_some(),
            "a non-DEM terrain prim must enter Avian with its authored support collider"
        );
        assert!(!live.3, "static terrain is not dynamic admission work");
    }

    #[test]
    fn omitted_mass_is_left_to_avian_computed_mass() {
        let source = FIXTURE.replace("    double physics:mass = 500\n", "");
        let stage = stage_from_source(&source);
        let view = stage.view();

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
            let stage = stage_from_source(&source);
            let view = stage.view();

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
            let stage = stage_from_source(&source);
            let view = stage.view();
            assert!(
                read_physics_material(&view, &SdfPath::new("/World/Ground").unwrap()).is_err(),
                "malformed material value in {name} must not disappear into Avian defaults"
            );
        }
    }

    #[test]
    fn standalone_static_colliders_receive_their_bound_physics_material() {
        let stage = stage_from_source(MATERIAL_FIXTURE);
        let view = stage.view();
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

#[cfg(test)]
mod joint_reader_tests {
    //! The joint projector reads the STANDARD UsdPhysics joint schema through
    //! the composed reader into the
    //! deferred `PendingUsdJoint` — bodies, axis, standard `physics:lowerLimit`/
    //! `upperLimit` (degrees → radians), local anchors, and `UsdPhysicsDriveAPI`.
    //! This is the headless-verifiable half of the rework (the read); joint
    //! *dynamics* need a rover boot.
    use super::read_joint_spec;
    use avian3d::prelude::MotorModel;
    use bevy::math::DVec3;
    use lunco_usd_avian_reader::joint::read_joint_spec_for_lint;
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    fn stage_from_source(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("test.usda", source))
            .expect("compose in-memory USDA fixture")
    }

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
        let stage = stage_from_source(FIXTURE);

        let j = read_joint_spec(&stage.view(), &SdfPath::new("/Hinge").unwrap())
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
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Hinge").unwrap())
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
        let stage = stage_from_source(RAKED);

        let j = read_joint_spec(&stage.view(), &SdfPath::new("/Spring").unwrap())
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
        let stage = stage_from_source("#usda 1.0\ndef Xform \"Plain\" {}\n");
        assert!(read_joint_spec(&stage.view(), &SdfPath::new("/Plain").unwrap()).is_none());
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
        let stage = lunco_usd_bevy_core::canonical::CanonicalStage::from_recipe(
            &lunco_usd_compose::recipe::StageRecipe::from_source("lint_only.usda", source),
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
                read_joint_spec(&stage.view(), &SdfPath::new("/Hinge").unwrap()).is_none(),
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
            read_joint_spec(&stage.view(), &SdfPath::new("/Hinge").unwrap()).is_none(),
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
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Ball").unwrap())
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
            read_joint_spec(&stage.view(), &SdfPath::new("/Generic").unwrap()).is_none(),
            "an unconstrained generic joint has multiple free DOFs and cannot reduce to fixed"
        );
    }

    /// A Z-up / centimetre stage — the Omniverse and Isaac Sim default — must
    /// convert the joint's AXIS and its AUTHORED anchors, exactly as meshes and
    /// colliders already do through their local transform composition.
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
        let stage = stage_from_source(ZUP_CM_FIXTURE);

        let j = read_joint_spec(&stage.view(), &SdfPath::new("/Hinge").unwrap())
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

    fn write_and_compose(_name: &str, body: &str) -> CanonicalStage {
        stage_from_source(body)
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
        let j = read_joint_spec(&stage.view(), &SdfPath::new("/Rover/Hinge").unwrap())
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
        let j = read_joint_spec(&stage.view(), &SdfPath::new("/Rover/Hinge").unwrap())
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
            &stage.view(),
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
            &stage.view(),
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
                &stage.view(),
                &SdfPath::new("/Host/Mount/YawJoint").unwrap()
            )
            .is_none(),
            "physics:jointEnabled = false must suppress the joint"
        );
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
                &stage.view(),
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
    use lunco_usd_bevy_core::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
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
        assert!(error.to_string().contains("/Root/Body"));
    }

    #[test]
    fn composite_compound_child_is_rejected_before_parry_compound_construction() {
        let source = r#"#usda 1.0
def Xform "Root" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] )
{
    def Mesh "Proxy" ( prepend apiSchemas = ["PhysicsCollisionAPI", "PhysicsMeshCollisionAPI"] )
    {
        point3f[] points = [(0,0,0),(2,0,0),(2,2,0),(0,2,0),(1,1,2)]
        int[] faceVertexCounts = [3,3,3,3]
        int[] faceVertexIndices = [0,1,4, 1,2,4, 2,3,4, 3,0,4]
        uniform token physics:approximation = "convexDecomposition"
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("nested.usda", source))
            .expect("build stage");
        let view = stage.view();
        let root = SdfPath::new("/Root").unwrap();
        let error = collect_child_colliders_from_usd(&view, &root)
            .expect_err("a composite child cannot be passed to Parry's flat Compound");
        assert!(error.to_string().contains("/Root/Proxy"));
        assert!(error.to_string().contains("composite runtime shape"));

        let (has_collider, body) = extract(&view, "/Root");
        assert!(
            !has_collider,
            "invalid compound admission must not insert a collider"
        );
        assert_eq!(
            body, None,
            "invalid compound admission must not insert a body"
        );
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
    fn extract(view: &lunco_usd_bevy_core::StageView<'_>, path: &str) -> (bool, Option<RigidBody>) {
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
        assert!(collect_child_colliders_from_usd(&view, &lander)
            .expect("valid transforms")
            .is_empty());
        assert!(build_collider_from_usd(&view, &lander)
            .expect("valid transform")
            .is_some());
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
                (root_id, scene.as_bytes().to_vec()),
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
        assert_eq!(live_shapes[0].0 .0, DVec3::ZERO);
        assert_eq!(live_shapes[1].0 .0, DVec3::new(0.0, 2.0, 0.0));

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
        assert_eq!(prepared_shapes[0].0 .0, DVec3::ZERO);
        assert_eq!(prepared_shapes[1].0 .0, DVec3::new(0.0, 2.0, 0.0));
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
