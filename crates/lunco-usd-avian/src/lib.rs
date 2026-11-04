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
//! - **CollisionAPI prims** → physics-only `Collider` children with source identity,
//!   attached through `ColliderOf` (no independent `RigidBody`)
//!
//! ## Mapped Attributes
//!
//! | USD Attribute | Avian3D Component | Notes |
//! |---|---|---|
//! | `PhysicsRigidBodyAPI` (parent) | `RigidBody::Dynamic` | ONE per compound assembly |
//! | `PhysicsCollisionAPI` | `Collider`, `ColliderOf`, `UsdColliderSource` | One native collider per collision prim |
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
//! The `On<Add<UsdPrimPath>>` observer fires when the entity is spawned, but the USD asset
//! may not be loaded yet (async loading). The `process_usd_avian_prims` system runs in the
//! `Update` schedule and retries every frame until the asset is available.

use avian3d::dynamics::joints::joint_graph::JointGraph;
use avian3d::dynamics::solver::islands::PhysicsIslands;
use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
mod gravity;
mod joint_frames;
mod mass;
mod source_contacts;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_usd_avian_core::report_physics_runtime_fault;
use lunco_usd_avian_filters::collision_groups::{CollisionGroupTable, CollisionGroupTables};
use lunco_usd_avian_filters::filtered_pairs as collision_filters;
use lunco_usd_bevy_scene::{
    UsdAnimated, UsdPreviewOnly, UsdPrimPath, UsdSceneProjected, UsdSceneRoot,
    UsdVisualProjectionSet, instance_key, is_preview_only,
};
use lunco_usd_bevy_stage::{
    TransformReadError, UsdInstanceMember, UsdInstanceRoot, UsdRead, UsdStageAsset, world_transform,
};
use openusd::sdf::Path as SdfPath;
use openusd_schemas::geom::Purpose;
// UsdPhysics attribute + API-schema names as CONSTANTS, from openusd's own schema
// module. Hand-written `"physics:…"` string literals are how `physics:friction`
// (an attribute UsdPhysics does not define) got invented and lived here for
// months: a typo in a `&str` compiles.
use lunco_usd_avian_contracts::{
    AuthoredInitialVelocity, PendingJointAdmission, PendingUsdJoint, ScenePhysicsOwned,
    ShouldBeDynamic, UsdPhysicsProjected,
};
use lunco_usd_avian_reader::{
    collider::{
        ColliderBuildOutcome, ColliderProjectionError, build_collider_from_usd,
        collect_body_colliders_from_usd,
    },
    joint::{has_rigid_body_ancestor, read_joint_spec},
    read_authored_bool_or_default, read_authored_real, read_authored_vec3,
};
use openusd_schemas::physics::tokens as ptok;

/// Standard composed centre of mass including the prim's world scale.
/// Reduced realizations share this interpretation with full native bodies.
pub fn mass_center_of_mass_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    path: &SdfPath,
) -> Result<Option<DVec3>, String> {
    mass::center_of_mass(reader, path)
}

pub mod material;
use material::read_physics_material;

/// Runtime evidence for one joint-shaped entity, consumed by the generic Rhai
/// USD lint. The fact is deliberately read-only; it does not create or repair
/// topology. `linked` is the projection's authoritative endpoint contract,
/// while `pending`, `native`, and `graph` show which lifecycle stage currently
/// owns the entity.
#[derive(Clone, Debug)]
pub struct RuntimeJointFact {
    /// Live entity in this World; transports lower its identity at their boundary.
    pub entity: Entity,
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
            .get::<avian3d::dynamics::joints::joint_graph::JointComponentId>()
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
            entity: entity.id(),
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
            .then(left.entity.to_bits().cmp(&right.entity.to_bits()))
    });
    facts
}

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
        let mut query = world.query_filtered::<Entity, (
            Or<(With<UsdPrimPath>, With<ScenePhysicsOwned>)>,
            Or<(
                With<bevy::ecs::entity_disabling::Disabled>,
                Without<bevy::ecs::entity_disabling::Disabled>,
            )>,
        )>();
        query.iter(world).collect()
    };
    if let Err(error) = prepare_physics_teardown(world, &scene_entities) {
        error!("scene physics teardown could not retire its Avian state: {error}");
        world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise("usd-physics-teardown", None, "active-scene", error);
    }
}

/// Retire physics derived from one USD stage before its projected entities are
/// replaced. Whole-source edits and other coarse stage rebuilds do not pass
/// through the scene-replacement schedule, so they must use this same graph-safe
/// owner before the generic projector despawns stage entities.
pub fn prepare_stage_projection_reset(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
) -> Result<(), String> {
    let mut stage_entities: EntityHashSet = {
        let mut query = world.query::<(Entity, &UsdPrimPath)>();
        query
            .iter(world)
            .filter(|(_, path)| path.stage_handle.id() == stage_id)
            .map(|(entity, _)| entity)
            .collect()
    };

    // Synthesized bodies and joints live below authored prims in the entity
    // hierarchy. Include those explicit physics owners even when they have no
    // USD path of their own.
    let hierarchy: Vec<(Entity, Entity)> = {
        let mut query = world.query::<(Entity, &ChildOf)>();
        query
            .iter(world)
            .map(|(child, parent)| (child, parent.parent()))
            .collect()
    };
    let mut descendants = stage_entities.clone();
    loop {
        let before = descendants.len();
        for (child, parent) in &hierarchy {
            if descendants.contains(parent) {
                descendants.insert(*child);
            }
        }
        if descendants.len() == before {
            break;
        }
    }
    for entity in descendants {
        if world.get::<ScenePhysicsOwned>(entity).is_some() {
            stage_entities.insert(entity);
        }
    }

    // World-anchor bodies have no hierarchy parent. Resolve them from the
    // admitted joint edges that touch this stage, so the edge and its private
    // endpoint are retired together.
    let graph_links: Vec<(Entity, Entity, Entity)> = world
        .get_resource::<JointGraph>()
        .map(|graph| {
            graph
                .graph()
                .all_edge_weights()
                .map(|edge| (edge.entity, edge.body1, edge.body2))
                .collect()
        })
        .unwrap_or_default();
    let touching_links: Vec<_> = graph_links
        .into_iter()
        .filter(|(joint, body0, body1)| {
            stage_entities.contains(joint)
                || stage_entities.contains(body0)
                || stage_entities.contains(body1)
        })
        .collect();
    for (joint, body0, body1) in &touching_links {
        for endpoint in [joint, body0, body1] {
            if world
                .get::<UsdPrimPath>(*endpoint)
                .is_some_and(|path| path.stage_handle.id() != stage_id)
            {
                return Err(format!(
                    "stage reset for {stage_id:?} intersects physics edge endpoint {endpoint:?} owned by another stage"
                ));
            }
        }
        if world.get::<ScenePhysicsOwned>(*joint).is_some() {
            stage_entities.insert(*joint);
        }
    }
    let stage_anchors: Vec<Entity> = touching_links
        .iter()
        .flat_map(|(_, body0, body1)| [*body0, *body1])
        .filter(|entity| {
            world.get::<ScenePhysicsOwned>(*entity).is_some()
                && world.get::<UsdPrimPath>(*entity).is_none()
        })
        .collect();
    stage_entities.extend(stage_anchors.iter().copied());

    prepare_physics_teardown(world, &stage_entities)?;

    for anchor in stage_anchors {
        if world.get_entity(anchor).is_ok() && world.get::<UsdPrimPath>(anchor).is_none() {
            world.despawn(anchor);
        }
    }
    Ok(())
}

/// Promote a local prim refresh to a stage reset when the subtree participates
/// in Avian state. Joints can connect bodies outside the hierarchy, so resetting
/// only the edited subtree would leave constraints attached to retired entity
/// identities.
pub fn subtree_requires_stage_projection_reset(
    world: &World,
    stage_id: AssetId<UsdStageAsset>,
    prim_path: &str,
) -> bool {
    let prefix = prim_path.trim_end_matches('/');
    // Geometry descendants contribute to compound bodies even when their own
    // collisionEnabled is false (a derived proxy may read that visual source).
    if world.iter_entities().any(|entity| {
        entity.contains::<RigidBody>()
            && entity.get::<UsdPrimPath>().is_some_and(|prim| {
                prim.stage_handle.id() == stage_id && prefix.starts_with(&format!("{}/", prim.path))
            })
    }) {
        return true;
    }
    let affected: EntityHashSet = world
        .iter_entities()
        .filter(|entity| {
            entity.get::<UsdPrimPath>().is_some_and(|prim| {
                prim.stage_handle.id() == stage_id
                    && (prim.path == prefix
                        || prefix == "/"
                        || prim.path.starts_with(&format!("{prefix}/")))
            })
        })
        .map(|entity| entity.id())
        .collect();

    // A guide or disabled shape can acquire physical participation through an
    // edit before it has a native body or collider. Its composed schemas still
    // belong to this owner and require the same graph-safe projection boundary.
    if world
        .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
        .and_then(|stages| stages.get(stage_id))
        .is_some_and(|stage| {
            let reader = stage.view();
            affected.iter().any(|entity| {
                world
                    .get::<UsdPrimPath>(*entity)
                    .and_then(|path| SdfPath::new(&path.path).ok())
                    .is_some_and(|path| {
                        reader.has_api_schema(&path, "PhysicsRigidBodyAPI")
                            || reader.has_api_schema(&path, "PhysicsCollisionAPI")
                    })
            })
        })
    {
        return true;
    }

    if affected.iter().any(|entity| {
        world.get::<RigidBody>(*entity).is_some()
            || world.get::<Collider>(*entity).is_some()
            || world
                .get::<avian3d::dynamics::joints::joint_graph::JointComponentId>(*entity)
                .is_some()
            || world
                .get::<lunco_physics::PhysicsJointLink>(*entity)
                .is_some()
            || world.get::<PendingUsdJoint>(*entity).is_some()
            || world.get::<PendingJointAdmission>(*entity).is_some()
    }) {
        return true;
    }

    world.get_resource::<JointGraph>().is_some_and(|graph| {
        graph.graph().all_edge_weights().any(|edge| {
            affected.contains(&edge.entity)
                || affected.contains(&edge.body1)
                || affected.contains(&edge.body2)
        })
    })
}

fn prepare_physics_teardown(
    world: &mut World,
    scene_entities: &EntityHashSet,
) -> Result<(), String> {
    let has_physics = scene_entities.iter().any(|entity| {
        world.get::<RigidBody>(*entity).is_some()
            || world.get::<Collider>(*entity).is_some()
            || world
                .get::<avian3d::dynamics::joints::joint_graph::JointComponentId>(*entity)
                .is_some()
            || world
                .get::<lunco_physics::PhysicsJointLink>(*entity)
                .is_some()
    });
    if !has_physics {
        return Ok(());
    }
    if world.get_resource::<PhysicsIslands>().is_none()
        || world.get_resource::<JointGraph>().is_none()
        || world.get_resource::<ContactGraph>().is_none()
    {
        return Err(
            "USD physics entities exist without Avian island, joint, and contact graph resources"
                .to_owned(),
        );
    }

    let graph_joints: Vec<Entity> = world
        .resource::<avian3d::dynamics::joints::joint_graph::JointGraph>()
        .graph()
        .all_edge_weights()
        .filter(|edge| {
            scene_entities.contains(&edge.entity)
                || scene_entities.contains(&edge.body1)
                || scene_entities.contains(&edge.body2)
        })
        .map(|edge| edge.entity)
        .collect();

    let joints: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, (
            With<avian3d::dynamics::joints::joint_graph::JointComponentId>,
            Or<(
                With<bevy::ecs::entity_disabling::Disabled>,
                Without<bevy::ecs::entity_disabling::Disabled>,
            )>,
        )>();
        query
            .iter(world)
            .filter(|entity| scene_entities.contains(entity) || graph_joints.contains(entity))
            .collect()
    };
    let colliders: Vec<Entity> = {
        let mut query = world.query_filtered::<Entity, (
            With<Collider>,
            Or<(
                With<bevy::ecs::entity_disabling::Disabled>,
                Without<bevy::ecs::entity_disabling::Disabled>,
            )>,
        )>();
        query
            .iter(world)
            .filter(|entity| {
                scene_entities.contains(entity)
                    || world
                        .get::<ColliderOf>(*entity)
                        .is_some_and(|owner| scene_entities.contains(&owner.body))
            })
            .collect()
    };

    // Native hooks retire constraints before the scene hierarchy is removed.
    lunco_usd_avian_joints::retire_native_joints(world, &joints)?;

    // Removing ColliderMarker is Avian's supported contact-graph removal path;
    // removing only Collider leaves its required marker alive until too late.
    for entity in colliders {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ColliderMarker>();
        }
    }

    Ok(())
}

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        // Whole-step timing is part of the shared physics contract. Install it
        // for windowed and headless hosts alike; the UI only publishes the
        // resulting sample into its workbench snapshot.
        if !app.is_plugin_added::<avian3d::diagnostics::PhysicsDiagnosticsPlugin>() {
            app.add_plugins(avian3d::diagnostics::PhysicsDiagnosticsPlugin);
        }
        if !app.is_plugin_added::<avian3d::diagnostics::PhysicsTotalDiagnosticsPlugin>() {
            app.add_plugins(avian3d::diagnostics::PhysicsTotalDiagnosticsPlugin);
        }
        app.init_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>();
        app.world_mut()
            .resource_mut::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
            .register(
                lunco_usd_bevy_core::live_edit::UsdLiveEditOwner::stage_projection_reset_owner(
                    "usd-avian.physics",
                    subtree_requires_stage_projection_reset,
                    prepare_stage_projection_reset,
                ),
            );
        // Installs joints parked by `attach_joint` — the USD path attaches
        // authored joints, so this app must be able to land them.
        app.add_plugins(lunco_usd_avian_joints::JointAttachPlugin);
        app.init_resource::<lunco_core::RuntimeFaults>();
        {
            use lunco_core::AppSceneExt;
            app.init_scene_resource::<gravity::PhysicsSceneGravityOwner>();
        }
        app.add_observer(gravity::project_initial_gravity);
        app.add_systems(
            PreUpdate,
            gravity::refresh_gravity
                .after(lunco_core::RuntimeCycleSet::Lifecycle)
                .before(lunco_physics::apply_physics_holds),
        );
        app.add_systems(
            PreUpdate,
            joint_frames::refresh_joint_frames
                .after(lunco_core::RuntimeCycleSet::Lifecycle)
                .before(lunco_physics::body_mass::PhysicsGeometryAdmission)
                .before(lunco_physics::apply_physics_holds),
        );
        app.add_systems(lunco_core::SceneTeardown, prepare_scene_physics_teardown);
        // `on_add_usd_prim`: eager observer for joint pending-state.
        // `process_usd_avian_prims`: observer on UsdSceneProjected — fires
        //   right after the USD structural projection translates each prim,
        //   so the stage and Transform exist. CPU visual meshes may still be
        //   streaming; mesh-backed terrain has its own pending-collider phase.
        // `build_usd_physics_joints`: runs after authored USD simulation
        //   projection and pose seeding, before native joint admission. It is
        //   lifecycle preparation and must not depend on a fixed physics step.
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

        app.configure_sets(
            Update,
            (UsdVisualProjectionSet.before(lunco_usd_avian_joints::JointPreparation),),
        );
        app.register_type::<ShouldBeDynamic>()
            .register_type::<collision_filters::SharedTireContact>()
            .register_type::<lunco_core::Mobility>()
            .add_observer(on_add_usd_prim)
            .add_observer(process_usd_avian_prims)
            // Projection systems publish body components through deferred
            // commands. Flush that projection before checking whether joint
            // work exists, then commit the typed native plan before the single
            // owner of solver-joint admission runs.
            .add_systems(
                Update,
                bevy::ecs::schedule::ApplyDeferred
                    .in_set(lunco_usd_avian_joints::JointPreparation)
                    .before(build_usd_physics_joints),
            )
            .add_systems(
                Update,
                (
                    build_usd_physics_joints.run_if(any_with_component::<PendingUsdJoint>),
                    bevy::ecs::schedule::ApplyDeferred,
                )
                    .chain()
                    .in_set(lunco_usd_avian_joints::JointPreparation),
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

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(), ColliderProjectionError> {
    match build_collider_from_usd(reader, sdf_path)? {
        ColliderBuildOutcome::Built(collider) => {
            if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
                return Err(ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail: "collider local bounds are not finite, ordered, or f32-representable"
                        .to_owned(),
                });
            }
            commands.entity(entity).try_insert((
                collider,
                lunco_usd_avian_contracts::UsdColliderSource(sdf_path.to_string()),
            ));
        }
        ColliderBuildOutcome::UnsupportedGeometry { type_name }
            if reader.has_api_schema(sdf_path, ptok::PHYSICS_COLLISION_API) =>
        {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: format!(
                    "{} has PhysicsCollisionAPI but `{}` is not supported collision geometry",
                    sdf_path,
                    type_name.as_deref().unwrap_or("unknown prim")
                ),
            });
        }
        ColliderBuildOutcome::UnsupportedGeometry { .. } => {}
        ColliderBuildOutcome::DeferredMeshAsset => {
            return Err(ColliderProjectionError::Backend {
                prim: sdf_path.to_string(),
                detail: "rigid-body collider cannot be deferred while an external mesh loads"
                    .to_owned(),
            });
        }
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
    commands.entity(entity).try_insert(UsdPhysicsProjected);
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
/// asset is available. The exact triangle mesh supports nonlinear sweeps.
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

        let collider = Collider::trimesh_from_mesh_with_config(
            mesh,
            avian3d::prelude::TrimeshFlags::FIX_INTERNAL_EDGES,
        );

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
                if let Some(path) = prim_path {
                    commands.entity(entity).try_insert(
                        lunco_usd_avian_contracts::UsdColliderSource(path.path.clone()),
                    );
                }
            }
            None => {
                warn!("[usd-avian] terrain mesh has no usable geometry — no collider built");
                commands.entity(entity).remove::<PendingTerrainCollider>();
            }
        }
    }
}

/// Deferred system that maps USD physics attributes to Avian3D components.
///
/// This system runs in the `Update` schedule and processes all `UsdPrimPath` entities
/// that haven't been marked with `UsdPhysicsProjected` yet.
///
/// Each `PhysicsRigidBodyAPI` prim owns one native body. Its authored collision
/// prims are realized as physics-only collider children linked by `ColliderOf`.
/// Source identity, material and filters belong to each collider; motion, mass
/// overrides and joints belong to the body. USD visual entities keep their
/// authored hierarchy without acquiring duplicate colliders.
///
/// Observer: fires once per entity, the moment the USD structural projection
/// translates the prim (signalled by inserting `UsdSceneProjected`). CPU visual
/// meshes may still be streaming; physics reads the worker-produced composed
/// projection plan and does not depend on `Mesh3d` or a live `Stage`.
/// By that point the plan is committed and the same reader contract used by
/// visual and simulation projection is available for physics components.
fn process_usd_avian_prims(
    trigger: On<Add<UsdSceneProjected>>,
    query: Query<(&UsdPrimPath, Option<&UsdInstanceMember>), Without<UsdPhysicsProjected>>,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    q_scene_root: Query<(), With<UsdSceneRoot>>,
    mount_state: Option<Res<lunco_core::SceneMountState>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_stage::canonical::CanonicalStages>,
    mut group_tables: ResMut<CollisionGroupTables>,
    mut commands: Commands,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    let entity = trigger.entity;
    let Ok((prim_path, instance_member)) = query.get(entity) else {
        return;
    };
    // A USD Editor preview shares the normal visual projection pipeline with
    // the live scene, but it is not a second physical world.  The preview root
    // is deliberately marked `UsdPreviewOnly`; walk to it before reading any
    // PhysicsRigidBodyAPI so Avian cannot admit a duplicate body or a duplicate
    // joint/collider graph into the simulation.
    if is_preview_only(entity, &q_child_of, &q_preview_only) {
        commands.entity(entity).try_insert(UsdPhysicsProjected);
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
    let _projection_span = bevy::log::info_span!(
        "usd_avian_project_prim",
        path = %prim_path.path,
    )
    .entered();

    let id = prim_path.stage_handle.id();
    let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
        return;
    };
    let (reader, _generation) = canonical.reader_for(id, stage_asset);
    bevy::log::debug!(
        "[canonical] avian extract from composed reader: {}",
        prim_path.path
    );

    // Collision groups are a STAGE-wide statement read one prim at a time, so the
    // table is resolved once per stage and cached; recomputing it per prim would
    // be quadratic in prim count on a scene that authors any group at all.
    let groups = {
        let _span = bevy::log::info_span!(
            "usd_avian_collision_groups",
            path = %prim_path.path,
        )
        .entered();
        group_tables.get_or_read(id, &reader)
    };
    let _span = bevy::log::info_span!(
        "usd_avian_extract_prim",
        path = %prim_path.path,
    )
    .entered();
    // Referenced instances retain source paths on their members. Scope stable
    // collider identity exactly as the joint owner scopes native constraints.
    let instance_path = instance_member.map_or("", |member| member.root_path.as_str());
    commands
        .entity(entity)
        .try_insert(lunco_physics::PhysicsOrderKey(format!(
            "{instance_path}|{sdf_path}"
        )));
    extract_avian_prim(
        &reader,
        entity,
        &sdf_path,
        groups,
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> Result<(f64, DVec3), String> {
    let convention = lunco_usd_bevy_stage::stage_convention(reader)
        .map_err(|error| format!("{sdf_path}: stage convention metadata is invalid: {error:?}"))?;
    let magnitude_attr = ptok::PHYSICS_GRAVITY_MAGNITUDE;
    let magnitude = match reader.real(sdf_path, magnitude_attr) {
        Some(value) if value.is_finite() && value < 0.0 => lunco_environment::EARTH_SURFACE_GRAVITY,
        Some(value) if value.is_finite() => {
            let converted = convention.length(value);
            if !converted.is_finite() {
                return Err(format!(
                    "{sdf_path}.{magnitude_attr}={value:?} is not finite after stage-unit conversion"
                ));
            }
            converted
        }
        Some(value) => {
            return Err(format!(
                "{sdf_path}.{magnitude_attr}={value:?} must be finite; a finite negative value selects Earth gravity"
            ));
        }
        None if !reader.has_authored_attribute(sdf_path, magnitude_attr) => {
            lunco_environment::EARTH_SURFACE_GRAVITY
        }
        None => {
            return Err(format!(
                "{sdf_path}.{magnitude_attr}={:?} has an unsupported authored value type",
                reader.attr_value(sdf_path, magnitude_attr)
            ));
        }
    };
    let direction_attr = ptok::PHYSICS_GRAVITY_DIRECTION;
    let raw_direction = match reader.vec3_f64(sdf_path, direction_attr) {
        Some(value) => DVec3::from_array(value),
        None if !reader.has_authored_attribute(sdf_path, direction_attr) => DVec3::ZERO,
        None => {
            return Err(format!(
                "{sdf_path}.{direction_attr}={:?} has an unsupported authored value type",
                reader.attr_value(sdf_path, direction_attr)
            ));
        }
    };
    if !raw_direction.is_finite() {
        return Err(format!(
            "{sdf_path}.{direction_attr}={raw_direction:?} must be finite"
        ));
    }
    let direction = if raw_direction == DVec3::ZERO {
        DVec3::NEG_Y
    } else {
        convention
            .dir_d(raw_direction)
            .try_normalize()
            .ok_or_else(|| {
                format!("{sdf_path}.{direction_attr}={raw_direction:?} must not be degenerate")
            })?
    };
    if !lunco_physics::avian_backend_vector_is_valid(direction * magnitude) {
        return Err(format!(
            "{sdf_path}.{magnitude_attr} produces acceleration {:?} m/s² outside the finite native range",
            direction * magnitude
        ));
    }
    Ok((magnitude, direction))
}

/// Map a single composed USD prim to its Avian physics components through the
/// shared reader boundary. Split out of the observer so the read body can be
/// driven directly by tests.
fn is_physics_joint_type(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
) -> bool {
    matches!(
        reader.type_name(sdf_path).as_deref(),
        Some(
            ptok::PHYSICS_JOINT
                | ptok::PHYSICS_FIXED_JOINT
                | ptok::PHYSICS_REVOLUTE_JOINT
                | ptok::PHYSICS_PRISMATIC_JOINT
                | ptok::PHYSICS_SPHERICAL_JOINT
                | ptok::PHYSICS_DISTANCE_JOINT
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    entity: Entity,
    sdf_path: &SdfPath,
    commands: &mut Commands,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
) -> bool {
    if !is_physics_joint_type(reader, sdf_path) {
        return false;
    }
    // Lint-only fixtures remain available to the linter without participating
    // in native topology or runtime faults.
    if !lunco_usd_avian_reader::joint::is_runtime_joint_enabled(reader, sdf_path) {
        return true;
    }
    if let Some(joint) = read_joint_spec(reader, sdf_path) {
        info!("USD joint topology pending native projection: {sdf_path} ({entity:?})");
        commands.entity(entity).try_insert((
            joint_frames::UsdJointFrames::from(&joint),
            joint,
            lunco_physics::PhysicsJointPending,
            lunco_physics::PhysicsJointTopologyPending,
        ));
    } else if reader.boolean(sdf_path, ptok::PHYSICS_JOINT_ENABLED) != Some(false) {
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
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
        commands.entity(entity).try_insert(UsdPhysicsProjected);
        return;
    }

    // `UsdPhysicsScene` — simulation-wide settings, of which this engine consumes
    // gravity. It is a SETTINGS prim, not a body: it has no transform and no
    // collider, so it is handled here and the body/collider reads below are
    // skipped entirely.
    if reader.type_name(sdf_path).as_deref() == Some(ptok::PHYSICS_SCENE) {
        commands
            .entity(entity)
            .try_insert((gravity::UsdPhysicsSceneSettings, UsdPhysicsProjected));
        return;
    }

    // `guide` geometry is annotation — a debug axis, a planned path, a sensor
    // cone. It is never physical, whatever schemas happen to be on it, so it is
    // refused a body and a collider both rather than being quietly collided with.
    let purpose = match reader.purpose(sdf_path) {
        Ok(purpose) => purpose,
        Err(detail) => {
            reject_collider_projection(
                commands,
                entity,
                sdf_path,
                faults,
                holds,
                ColliderProjectionError::Backend {
                    prim: sdf_path.to_string(),
                    detail,
                },
            );
            return;
        }
    };
    if purpose == Purpose::Guide {
        commands.entity(entity).try_insert(UsdPhysicsProjected);
        return;
    }

    // `PhysicsFilteredPairsAPI` applies to a body OR to a collider under one, and
    // is read before either branch below because it is orthogonal to both: it says
    // which pairs never collide, not what this prim IS.
    if let Some(pending) = collision_filters::read_filtered_pairs(reader, sdf_path) {
        commands.entity(entity).try_insert(pending);
    }

    // Wheel realization belongs to the sim plugin. Standard collision filters
    // remain owned here and must reach specialized colliders as well as bodies
    // projected below, before either path opens physics admission.
    if reader
        .real_f32(sdf_path, "physxVehicleWheel:radius")
        .is_some()
    {
        apply_collision_groups(commands, entity, groups, sdf_path);
        commands.entity(entity).try_insert(UsdPhysicsProjected);
        return;
    }

    let has_rigid_body_api = reader.has_api_schema(sdf_path, ptok::PHYSICS_RIGID_BODY_API);
    let has_collision_api = reader.has_api_schema(sdf_path, ptok::PHYSICS_COLLISION_API);
    let has_terrain_api = reader.has_api_schema(sdf_path, "LunCoTerrainAPI");
    // ── TERRAIN ── static collider; the terrain projection owns its marker.
    if has_terrain_api {
        if apply_physics_material(commands, entity, reader, sdf_path).is_err() {
            error!(
                "[usd-avian] {sdf_path} has malformed physics-material values — refusing terrain projection"
            );
            commands.entity(entity).try_insert(UsdPhysicsProjected);
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
                Ok(ColliderBuildOutcome::Built(collider)) => {
                    if lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
                        commands.entity(entity).try_insert((
                            collider,
                            lunco_usd_avian_contracts::UsdColliderSource(sdf_path.to_string()),
                        ));
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
                Ok(ColliderBuildOutcome::DeferredMeshAsset) => {
                    commands.entity(entity).try_insert(PendingTerrainCollider);
                }
                Ok(ColliderBuildOutcome::UnsupportedGeometry { type_name }) => {
                    reject_collider_projection(
                        commands,
                        entity,
                        sdf_path,
                        faults.as_deref_mut(),
                        holds.as_deref_mut(),
                        ColliderProjectionError::Backend {
                            prim: sdf_path.to_string(),
                            detail: format!(
                                "terrain prim has no supported collider geometry (type: {})",
                                type_name.as_deref().unwrap_or("unknown")
                            ),
                        },
                    );
                    return;
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
        commands.entity(entity).try_insert(UsdPhysicsProjected);
        return;
    }

    // A standalone collision prim has no body whose compound reader can filter
    // its geometry. Exclude disabled shapes before material or collider work.
    // A rigid body's own flag filters only that shape, not its child shapes.
    if has_collision_api && !has_rigid_body_api {
        match read_authored_bool_or_default(reader, sdf_path, ptok::PHYSICS_COLLISION_ENABLED, true)
        {
            Ok(true) => {}
            Ok(false) => {
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
            Err(()) => {
                reject_collider_projection(
                    commands,
                    entity,
                    sdf_path,
                    faults.as_deref_mut(),
                    holds.as_deref_mut(),
                    ColliderProjectionError::Backend {
                        prim: sdf_path.to_string(),
                        detail: format!("malformed {}", ptok::PHYSICS_COLLISION_ENABLED),
                    },
                );
                return;
            }
        }
    }

    // ── TRIGGER SENSOR ── applying the API opts into an overlap-only static
    // Sensor; `lunco:triggerZone` is an optional label for named geofence events.
    if reader.has_api_schema(sdf_path, "LunCoTriggerZoneAPI") {
        let zone = match reader.text(sdf_path, "lunco:triggerZone") {
            Some(zone) => zone,
            None if !reader.has_authored_attribute(sdf_path, "lunco:triggerZone") => String::new(),
            None => {
                reject_collider_projection(
                    commands,
                    entity,
                    sdf_path,
                    faults.as_deref_mut(),
                    holds.as_deref_mut(),
                    ColliderProjectionError::Backend {
                        prim: sdf_path.to_string(),
                        detail: "malformed lunco:triggerZone; expected a token".to_owned(),
                    },
                );
                return;
            }
        };
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
        commands.entity(entity).try_insert(UsdPhysicsProjected);
        return;
    }

    if has_rigid_body_api {
        // Prepare complete validated native caches before inserting RigidBody:
        // Avian's synchronous insertion observer must never construct computed
        // properties from unchecked intermediate geometry or mass rescaling.
        let simulated = match read_authored_bool_or_default(
            reader,
            sdf_path,
            ptok::PHYSICS_RIGID_BODY_ENABLED,
            true,
        ) {
            Ok(value) => value,
            Err(()) => {
                error!(
                    "[usd-avian] {sdf_path} has malformed {} — refusing rigid-body projection",
                    ptok::PHYSICS_RIGID_BODY_ENABLED
                );
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
        };
        // Thin, falling collision geometry needs a swept trajectory, not only
        // overlap at the end of a step. Honor the authored PhysX schema flag
        // through Avian's backend; no vehicle-type inference is involved.
        let ccd = match read_authored_bool_or_default(
            reader,
            sdf_path,
            "physxRigidBody:enableCCD",
            false,
        ) {
            Ok(value) => value,
            Err(()) => {
                error!(
                    "[usd-avian] {sdf_path} has malformed physxRigidBody:enableCCD — refusing rigid-body projection"
                );
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
        };
        let kinematic = match read_authored_bool_or_default(
            reader,
            sdf_path,
            ptok::PHYSICS_KINEMATIC_ENABLED,
            false,
        ) {
            Ok(value) => value,
            Err(()) => {
                error!(
                    "[usd-avian] {sdf_path} has malformed {} — refusing rigid-body projection",
                    ptok::PHYSICS_KINEMATIC_ENABLED
                );
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
        };

        // Prepare the complete body geometry before admitting any of its native
        // colliders. Each USD source retains its own collider identity.
        let body_colliders = {
            let _span = bevy::log::info_span!(
                "usd_avian_body_colliders",
                path = %sdf_path,
            )
            .entered();
            collect_body_colliders_from_usd(reader, sdf_path)
        };
        let body_colliders = match body_colliders {
            Ok(shapes) => shapes,
            Err(error) => {
                reject_collider_projection(commands, entity, sdf_path, faults, holds, error);
                return;
            }
        };
        let body_scale = match lunco_usd_bevy_stage::world_transform_matrix_d_at(
            reader, sdf_path, 0.0,
        ) {
            Ok(matrix)
                if matrix.determinant().is_finite()
                    && matrix.determinant() != 0.0
                    && matrix.row(3).abs_diff_eq(bevy::math::DVec4::W, 1e-6) =>
            {
                let (scale, _, _) = matrix.to_scale_rotation_translation();
                let native = scale.as_vec3();
                if !native.is_finite() || !native.cmpne(Vec3::ZERO).all() {
                    report_physics_runtime_fault(
                        faults,
                        holds,
                        entity,
                        sdf_path.to_string(),
                        "usd-rigid-body-mass-invalid",
                        format!(
                            "body world scale {scale:?} exceeds the native collider representation"
                        ),
                    );
                    commands.entity(entity).try_insert(UsdPhysicsProjected);
                    return;
                }
                native
            }
            _ => {
                report_physics_runtime_fault(
                    faults,
                    holds,
                    entity,
                    sdf_path.to_string(),
                    "usd-rigid-body-mass-invalid",
                    "body mass requires a nonsingular affine world transform".to_owned(),
                );
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
        };
        let mut prepared_colliders = Vec::with_capacity(body_colliders.len());
        let mut geometry_mass = Vec::with_capacity(body_colliders.len());
        for part in body_colliders {
            let source_path = SdfPath::new(&part.source_path)
                .expect("collider source was validated by the composed reader");
            let prepared = read_physics_material(reader, &source_path)
                .map_err(|error| error.to_string())
                .and_then(|material| {
                    collider_density_from_usd(reader, &source_path, sdf_path)
                        .map(|density| (material, density))
                });
            let (material, density) = match prepared {
                Ok(prepared) => prepared,
                Err(detail) => {
                    reject_collider_projection(
                        commands,
                        entity,
                        sdf_path,
                        faults,
                        holds,
                        ColliderProjectionError::Backend {
                            prim: source_path.to_string(),
                            detail,
                        },
                    );
                    return;
                }
            };
            let overrides = if source_path == *sdf_path {
                Ok(lunco_physics::body_mass::ColliderMassOverrides::default())
            } else {
                mass::overrides(reader, &source_path)
            };
            let overrides = overrides.and_then(|mut overrides| {
                let basis = lunco_usd_bevy_scene::read_geometry_orientation(reader, &source_path)
                    .ok_or_else(|| "invalid collider geometry basis".to_owned())?
                    .inverse();
                if let Some(center) = overrides.center {
                    overrides.center = Some(basis * center);
                }
                if overrides.principal.is_some() {
                    overrides.principal_axes = Some(
                        basis
                            * overrides
                                .principal_axes
                                .unwrap_or(bevy::math::DQuat::IDENTITY),
                    );
                } else if let Some(axes) = overrides.principal_axes {
                    overrides.principal_axes = Some(basis * axes * basis.inverse());
                    overrides.rotate_geometry_frame = true;
                }
                Ok(overrides)
            });
            let properties = overrides.and_then(|overrides| {
                let mut world_collider = part.collider.clone();
                // Match Avian's maintained collider-scale cooking boundary.
                world_collider.set_scale(body_scale, 10);
                if world_collider.scale() != body_scale {
                    return Err("native collider could not apply the body world scale".to_owned());
                }
                if !lunco_physics::avian_backend_collider_shape_is_valid(&world_collider)
                    || !lunco_physics::avian_backend_point_is_valid(
                        part.position.0 * body_scale.as_dvec3(),
                    )
                {
                    return Err(
                        "body-scaled collider geometry or offset exceeds the native representation"
                            .to_owned(),
                    );
                }
                let geometry = world_collider
                    .shape_scaled()
                    .mass_properties(f64::from(density.0));
                overrides
                    .apply(geometry)
                    .map(|properties| (overrides, properties))
                    .map_err(str::to_owned)
            });
            let (overrides, properties) = match properties {
                Ok(properties) => properties,
                Err(detail) => {
                    report_physics_runtime_fault(
                        faults,
                        holds,
                        entity,
                        source_path.to_string(),
                        "usd-rigid-body-mass-invalid",
                        detail,
                    );
                    commands.entity(entity).try_insert(UsdPhysicsProjected);
                    return;
                }
            };
            geometry_mass.push(
                properties.transform_by(&avian3d::parry::math::Pose::from_parts(
                    part.position.0 * body_scale.as_dvec3(),
                    part.rotation.0.as_dquat(),
                )),
            );
            prepared_colliders.push((part, source_path, material, density, overrides));
        }
        let geometry = match lunco_physics::body_mass::sum_mass_properties(geometry_mass) {
            Ok(properties) => properties,
            Err(detail) => {
                report_physics_runtime_fault(
                    faults,
                    holds,
                    entity,
                    sdf_path.to_string(),
                    "usd-rigid-body-mass-invalid",
                    detail.to_owned(),
                );
                commands.entity(entity).try_insert(UsdPhysicsProjected);
                return;
            }
        };
        let automatic =
            match apply_rigid_body_mass_props(commands, entity, reader, sdf_path, geometry) {
                Ok(automatic) => automatic,
                Err(detail) => {
                    report_physics_runtime_fault(
                        faults,
                        holds,
                        entity,
                        sdf_path.to_string(),
                        "usd-rigid-body-mass-invalid",
                        detail,
                    );
                    commands.entity(entity).try_insert(UsdPhysicsProjected);
                    return;
                }
            };
        apply_collision_groups(commands, entity, groups, sdf_path);
        commands
            .entity(entity)
            .try_insert(lunco_physics::continuous_collision_components(
                ccd && simulated && !kinematic,
            ));

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
        commands.entity(entity).try_insert((
            body,
            mobility,
            lunco_core::SelectableRoot,
            lunco_physics::body_mass::BodyMassFromColliders {
                mass: automatic.0,
                inertia: automatic.1,
                center: automatic.2,
            },
        ));
        // The native observer sees complete validated caches. The application
        // geometry owner retains automatic fields through typed ownership.
        for (part, source_path, material, density, overrides) in prepared_colliders {
            let mut collider = commands.spawn((
                Name::new(format!("USD collider {}", part.source_path)),
                Transform::from_translation(part.position.0.as_vec3())
                    .with_rotation(part.rotation.0),
                GlobalTransform::default(),
                ChildOf(entity),
                ColliderOf { body: entity },
                ScenePhysicsOwned,
                lunco_usd_avian_contracts::UsdColliderSource(part.source_path.clone()),
            ));
            let collider_entity = collider.id();
            // All physics admission is applied in one command flush. The body's
            // exact instance-scoped key has already been queued by its projector.
            let source = part.source_path;
            collider.queue(move |mut child: EntityWorldMut| {
                let key = child
                    .world()
                    .get::<lunco_physics::PhysicsOrderKey>(entity)
                    .map(|key| key.collider_source(&source));
                let hooks = child.world().get::<ActiveCollisionHooks>(entity).copied();
                if let Some(key) = key {
                    child.insert(key);
                }
                if let Some(hooks) = hooks {
                    child.insert(hooks);
                }
                child.world_scope(|world| {
                    source_contacts::bind_native_collider(world, collider_entity)
                });
            });
            commands
                .entity(collider_entity)
                .try_insert((density, overrides));
            if let Some(mass) = overrides.mass {
                commands
                    .entity(collider_entity)
                    .try_insert(Mass(mass as f32));
            }
            if let Some(principal) = overrides.principal {
                commands
                    .entity(collider_entity)
                    .try_insert(AngularInertia::new_with_local_frame(
                        principal.as_vec3(),
                        overrides
                            .principal_axes
                            .unwrap_or(bevy::math::DQuat::IDENTITY)
                            .as_quat(),
                    ));
            }
            if let Some(center) = overrides.center {
                commands
                    .entity(collider_entity)
                    .try_insert(CenterOfMass(center.as_vec3()));
            }
            apply_collision_groups(commands, collider_entity, groups, &source_path);
            insert_physics_material(commands, collider_entity, material);
            commands.entity(collider_entity).try_insert(part.collider);
        }
        if !simulated || kinematic {
            commands
                .entity(entity)
                .try_insert(lunco_core::PhysicsStateReady);
        }

        commands.entity(entity).try_insert(UsdPhysicsProjected);
    } else if has_collision_api {
        // ── COLLIDER PRIM, no body of its own ──
        // Per the USD physics spec, a collider belongs to the nearest ancestor
        // carrying `PhysicsRigidBodyAPI`, which owns its native collider child. Only when NO ancestor is a
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
                commands.entity(entity).try_insert(UsdPhysicsProjected);
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
        } else {
            commands
                .queue(move |world: &mut World| source_contacts::bind_source_prim(world, entity));
        }
        commands.entity(entity).try_insert(UsdPhysicsProjected);
    } else {
        // Neither a body nor a collider: no physics components, only the marker.
        commands.entity(entity).try_insert(UsdPhysicsProjected);
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
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
    trigger: On<Add<UsdPrimPath>>,
    query: Query<&UsdPrimPath>,
    q_child_of: Query<&ChildOf>,
    q_preview_only: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_stage::canonical::CanonicalStages>,
    mut commands: Commands,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else {
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
    let (reader, _generation) = canonical.reader_for(id, stage_asset);
    if reader
        .real_f32(&sdf_path, "physxVehicleWheel:radius")
        .is_some()
    {
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

/// Resolves pending USD joints after their projected endpoints and poses are
/// available. Authored relationship errors are reported by `read_joint_spec`;
/// valid relationships stay pending while bounded scene projection publishes
/// their body entities.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingUsdJointWait {
    MissingBody { body0: bool, body1: bool },
    MissingPoseOwner { body0: bool, body1: bool },
    UnseededPose { body0: bool, body1: bool },
}

#[derive(Default)]
struct PendingUsdJointResolutionState {
    waits: EntityHashMap<PendingUsdJointWait>,
}

fn report_pending_usd_joint_wait(
    waits: &mut EntityHashMap<PendingUsdJointWait>,
    joint_entity: Entity,
    joint_path: &str,
    reason: PendingUsdJointWait,
    detail: impl FnOnce() -> String,
) {
    if waits.get(&joint_entity) != Some(&reason) {
        waits.insert(joint_entity, reason);
        info!("[usd-avian] joint {joint_path} is waiting: {}", detail());
    }
}

fn build_usd_physics_joints(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingUsdJoint, &UsdPrimPath)>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    // Preparation may run while the world-readiness hold has paused Avian's
    // nested PhysicsSchedule. These filters establish that both USD endpoints
    // are live rigid bodies with a Position slot. They may still carry
    // `RigidBodyDisabled` while readiness freezes the authored subtree; that
    // marker means "do not integrate yet", not "the authored body does not
    // exist". They do NOT claim island admission. `attach_joint` parks the
    // constraint as `PendingJoint`, and `lunco-usd-avian-joints::JointAttachPlugin`
    // is the sole owner of admitting that parked constraint after Avian creates
    // both island nodes.
    //
    // `Position` is still only pose storage until `q_shadow` below confirms the
    // bridge has seeded it. Keeping those two facts separate prevents seating
    // against the required-component default at the origin.
    q_bodies: Query<(Entity, &UsdPrimPath), (With<RigidBody>, With<Position>)>,
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
    q_instance_member: Query<&UsdInstanceMember>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut resolution: Local<PendingUsdJointResolutionState>,
) {
    resolution.waits.retain(|e, _| q_pending.contains(*e));
    // Pending constraints are created from deferred USD projections.  ECS
    // iteration order is not authored order and can vary when async referenced
    // layers finish on different frames.  Attach them by stable USD path so
    // native solver-island construction cannot depend on loader timing.
    let mut pending_joints: Vec<_> = q_pending.iter().collect();
    pending_joints.sort_by(|left, right| left.2.path.cmp(&right.2.path));
    for (joint_entity, pending, joint_prim_path) in pending_joints {
        // Joint preparation is intentionally allowed while world readiness
        // holds. The hold pauses integration, not topology construction:
        // the native joint-admission package parks the constraint and admits it
        // after Avian creates the solver body-island nodes. Holding
        // this builder would deadlock readiness because the binding epoch waits
        // for the pending joint marker to clear.
        let joint_root = instance_key(joint_entity, &q_provenance, &q_gid, &q_instance_root);
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
                    && instance_key(*e, &q_provenance, &q_gid, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);
        let body1_ent = q_bodies
            .iter()
            .find(|(e, path)| {
                path.path == pending.body1_path
                    && path.stage_handle == joint_prim_path.stage_handle
                    && instance_key(*e, &q_provenance, &q_gid, &q_instance_root) == joint_root
            })
            .map(|(e, _)| e);

        let missing0 = !world0 && body0_ent.is_none();
        let missing1 = !world1 && body1_ent.is_none();
        if missing0 || missing1 {
            report_pending_usd_joint_wait(
                &mut resolution.waits,
                joint_entity,
                &joint_prim_path.path,
                PendingUsdJointWait::MissingBody {
                    body0: missing0,
                    body1: missing1,
                },
                || {
                    format!(
                        "body endpoint projection (body0='{}', missing={missing0}; body1='{}', missing={missing1})",
                        pending.body0_path, pending.body1_path
                    )
                },
            );
            continue;
        }

        // `Position` is pose storage, not proof that the bridge seeded an
        // authored pose. The bridge publishes this fact in `PreUpdate`, before
        // joint preparation in `Update`; an unseeded endpoint remains pending
        // rather than being seated against a required-component default.
        //
        // `BridgeShadow::is_seeded` is the honest signal: the shadow starts as a NaN
        // sentinel and becomes finite exactly when the bridge first writes a real
        // world pose. A missing shadow is not a readiness signal: unless the
        // ground-placement owner supplied an authoritative pose, the physics
        // pose owner is absent and this joint must not be seated.
        //
        let seeded = |e: Entity| {
            q_pose_authoritative.contains(e)
                || q_shadow
                    .get(e)
                    .is_ok_and(lunco_usd_avian_core::BridgeShadow::is_seeded)
        };
        let missing_pose_owner =
            |e: Entity| !q_pose_authoritative.contains(e) && q_shadow.get(e).is_err();
        let body0_missing_pose_owner = body0_ent.is_some_and(missing_pose_owner);
        let body1_missing_pose_owner = body1_ent.is_some_and(missing_pose_owner);
        if body0_missing_pose_owner || body1_missing_pose_owner {
            let reason = PendingUsdJointWait::MissingPoseOwner {
                body0: body0_missing_pose_owner,
                body1: body1_missing_pose_owner,
            };
            if resolution.waits.get(&joint_entity) != Some(&reason) {
                let detail = format!(
                    "physics pose owner is missing for body0='{}' ({}) or body1='{}' ({})",
                    pending.body0_path,
                    if body0_missing_pose_owner {
                        "missing"
                    } else {
                        "available"
                    },
                    pending.body1_path,
                    if body1_missing_pose_owner {
                        "missing"
                    } else {
                        "available"
                    },
                );
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "usd-joint-pose-owner-missing",
                        Some(joint_entity),
                        joint_prim_path.path.clone(),
                        detail.clone(),
                    );
                }
                if let Some(holds) = holds.as_deref_mut() {
                    holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                }
            }
            report_pending_usd_joint_wait(
                &mut resolution.waits,
                joint_entity,
                &joint_prim_path.path,
                reason,
                || format!("{reason:?}"),
            );
            continue;
        }
        let body0_unseeded = body0_ent.is_some_and(|e| !seeded(e));
        let body1_unseeded = body1_ent.is_some_and(|e| !seeded(e));
        if body0_unseeded || body1_unseeded {
            report_pending_usd_joint_wait(
                &mut resolution.waits,
                joint_entity,
                &joint_prim_path.path,
                PendingUsdJointWait::UnseededPose {
                    body0: body0_unseeded,
                    body1: body1_unseeded,
                },
                || {
                    format!(
                        "physics pose seeding (body0='{}', waiting={body0_unseeded}; body1='{}', waiting={body1_unseeded})",
                        pending.body0_path, pending.body1_path
                    )
                },
            );
            continue;
        }

        let resolved_drive_model = match pending.drive {
            None => None,
            Some(drive) => match drive.motor_model() {
                Ok(model) => Some(model),
                Err(error) => {
                    let detail = format!("standard USD drive has invalid parameters: {error:?}");
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
                        .remove::<lunco_physics::PhysicsJointPending>()
                        .remove::<lunco_physics::PhysicsJointTopologyPending>();
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
        if (world0 || world1) && active_frame.is_none() {
            report_physics_runtime_fault(
                faults.as_deref_mut(),
                holds.as_deref_mut(),
                joint_entity,
                joint_prim_path.path.clone(),
                "usd-joint-world-frame-missing",
                "World-anchored joint has no admitted active physics frame".to_owned(),
            );
            continue;
        }
        let mut world_anchor = || {
            let frame = active_frame
                .as_ref()
                .expect("validated world endpoint frame")
                .0;
            commands
                .spawn((
                    RigidBody::Static,
                    ScenePhysicsOwned,
                    lunco_core::SceneOwned,
                    lunco_core::PhysicsPoseAuthoritative,
                    Position(DVec3::ZERO),
                    Rotation::default(),
                    Transform::IDENTITY,
                    ChildOf(frame),
                ))
                .id()
        };
        let b0 = body0_ent.unwrap_or_else(&mut world_anchor);
        let b1 = body1_ent.unwrap_or_else(&mut world_anchor);
        let instance_path = q_instance_member
            .get(joint_entity)
            .map(|member| member.root_path.as_str())
            .unwrap_or("");
        let order_key = format!("{instance_path}|{}", joint_prim_path.path);

        debug!(
            "Built USD joint {} -> {} <-> {}",
            pending.joint_type, pending.body0_path, pending.body1_path,
        );

        // Seating runs at the pending-joint admission boundary, after the body
        // states exist and before the next solver step.
        // Put the avian joint component ON the joint prim entity itself (it
        // already carries `UsdPrimPath` + the loader-assigned `GlobalEntityId`)
        // rather than spawning a fresh anonymous entity. This makes the joint
        // — and the `angle` port `lunco-cosim` auto-exposes on any
        // `RevoluteJoint` — addressable by USD path, API id, or `Entity` alike,
        // so the wiring fabric can target `</…/Joint>.angle` with no
        // USD-specific lookup.
        let attached = match pending.joint_type.as_str() {
            "PhysicsPrismaticJoint" => {
                let motor = pending.drive.map(|d| LinearMotor {
                    enabled: d.is_active(),
                    target_position: d.target_position.unwrap_or(0.0) as f32,
                    target_velocity: d.target_velocity.unwrap_or(0.0) as f32,
                    max_force: d.max_force.unwrap_or(f64::INFINITY) as f32,
                    motor_model: resolved_drive_model.expect("resolved USD prismatic drive motor"),
                });
                lunco_usd_avian_joints::attach_prismatic_joint(
                    &mut commands,
                    joint_entity,
                    order_key.clone(),
                    b0,
                    b1,
                    pending.local_pos0,
                    pending.local_pos1,
                    pending.local_rot0,
                    pending.local_rot1,
                    pending.axis,
                    pending.limit_lower,
                    pending.limit_upper,
                    motor,
                );
                true
            }
            "PhysicsRevoluteJoint" => {
                let motor = pending.drive.map(|d| AngularMotor {
                    enabled: d.is_active(),
                    target_position: d.target_position.unwrap_or(0.0) as f32,
                    target_velocity: d.target_velocity.unwrap_or(0.0) as f32,
                    max_torque: d.max_force.unwrap_or(f64::INFINITY) as f32,
                    motor_model: resolved_drive_model.expect("resolved USD revolute drive motor"),
                });
                lunco_usd_avian_joints::attach_revolute_joint(
                    &mut commands,
                    joint_entity,
                    order_key.clone(),
                    b0,
                    b1,
                    pending.local_pos0,
                    pending.local_pos1,
                    pending.local_rot0,
                    pending.local_rot1,
                    pending.axis,
                    pending.limit_lower,
                    pending.limit_upper,
                    motor,
                );
                true
            }
            "PhysicsFixedJoint" => {
                lunco_usd_avian_joints::attach_fixed_joint(
                    &mut commands,
                    joint_entity,
                    order_key.clone(),
                    b0,
                    b1,
                    pending.local_pos0,
                    pending.local_pos1,
                    pending.local_rot0,
                    pending.local_rot1,
                );
                true
            }
            "PhysicsSphericalJoint" => {
                // Ball joint: 3 rotational DOF about the anchor. The axis is the
                // twist axis; the cone limits bound swing and the lower/upper
                // limits bound twist.
                lunco_usd_avian_joints::attach_spherical_joint(
                    &mut commands,
                    joint_entity,
                    order_key.clone(),
                    b0,
                    b1,
                    pending.local_pos0,
                    pending.local_pos1,
                    pending.local_rot0,
                    pending.local_rot1,
                    pending.axis,
                    pending.swing_limit,
                    pending.limit_lower,
                    pending.limit_upper,
                );
                true
            }
            "PhysicsDistanceJoint" => {
                // Tether/strut: keeps the two anchors within [min, max] distance.
                // A negative or unauthored distance disables that bound.
                lunco_usd_avian_joints::attach_distance_joint(
                    &mut commands,
                    joint_entity,
                    order_key.clone(),
                    b0,
                    b1,
                    pending.local_pos0,
                    pending.local_pos1,
                    pending.limit_lower,
                    pending.limit_upper,
                );
                true
            }
            // Generic D6 has no Avian primitive. Reducing it needs per-DOF
            // PhysicsLimitAPI analysis; explicit joint kinds are required.
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
            if resolution.waits.remove(&joint_entity).is_some() {
                info!(
                    "[usd-avian] joint {} wait cleared after native topology projection",
                    joint_prim_path.path
                );
            }
            if let Some(damping) = pending.damping {
                commands.entity(joint_entity).try_insert(damping);
            }
            commands
                .entity(joint_entity)
                .try_remove::<lunco_physics::PhysicsJointTopologyPending>();
        }

        commands.entity(joint_entity).remove::<PendingUsdJoint>();
    }
}

/// Resolve density from the nearest authored MassAPI within this body, then
/// the source's inherited physics material. Stage length units are cubed once.
fn collider_density_from_usd(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    source: &SdfPath,
    body: &SdfPath,
) -> Result<ColliderDensity, String> {
    let mut path = source.to_string();
    let mut density = None;
    loop {
        let prim = SdfPath::new(&path).map_err(|error| error.to_string())?;
        match read_authored_real(reader, &prim, ptok::PHYSICS_DENSITY)
            .map_err(|()| format!("{prim}.physics:density has an unreadable authored type"))?
        {
            None | Some(0.0) => {}
            Some(value) if value.is_finite() && value > 0.0 => {
                density = Some(value);
                break;
            }
            Some(value) => {
                return Err(format!(
                    "{prim}.physics:density={value:?} must be finite and positive, or zero for automatic density"
                ));
            }
        }
        if path == body.as_str() {
            break;
        }
        path = path
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_owned())
            .ok_or_else(|| "collider density ancestry does not reach its body".to_owned())?;
        if path.is_empty() {
            return Err("collider density ancestry does not reach its body".to_owned());
        }
    }
    if density.is_none() {
        density = read_physics_material(reader, source)
            .map_err(|error| error.to_string())?
            .and_then(|material| material.density)
            .filter(|density| *density > 0.0);
    }
    let scale = lunco_usd_bevy_stage::stage_convention(reader)
        .map_err(|error| error.to_string())?
        .length(1.0);
    // The normative omitted density is already expressed in canonical SI.
    let density = density.map(|value| value / scale.powi(3)).unwrap_or(1000.0);
    if !density.is_finite() || density <= 0.0 || density > f32::MAX as f64 {
        return Err(format!(
            "{source}.physics:density resolves to {density:?} kg/m³, outside the finite positive native range"
        ));
    }
    let native = density as f32;
    if native <= 0.0 || !native.recip().is_finite() {
        return Err(format!(
            "{source}.physics:density={density:?} kg/m³ cannot form a finite positive native density and reciprocal"
        ));
    }
    Ok(ColliderDensity(native))
}

fn apply_rigid_body_mass_props(
    commands: &mut Commands,
    entity: Entity,
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    sdf_path: &SdfPath,
    geometry: avian3d::parry::mass_properties::MassProperties,
) -> Result<(bool, bool, bool), String> {
    let conv = lunco_usd_bevy_stage::stage_convention(reader).map_err(|error| error.to_string())?;
    let mpu = conv.length(1.0);
    if !mpu.is_finite() || mpu <= 0.0 {
        return Err("stage metersPerUnit must resolve to a finite positive length".to_owned());
    }
    let overrides = mass::overrides(reader, sdf_path)?;
    let has_volume = geometry.mass() > 0.0;
    let candidate = mass::body_candidate(geometry, overrides).map_err(str::to_owned)?;
    let native = lunco_physics::body_mass::native_mass_properties(&candidate).map_err(|reason| format!(
        "physics:mass={:?}, physics:diagonalInertia={:?}, physics:centerOfMass={:?}: {reason}",
        overrides.mass, overrides.principal, overrides.center,
    ))?;
    let linear_damping = match read_authored_real(reader, sdf_path, PHYSX_LINEAR_DAMPING)
        .map_err(|()| format!("{} has an unreadable authored value", PHYSX_LINEAR_DAMPING))?
    {
        None => None,
        Some(value) if value.is_finite() && value >= 0.0 && value <= f64::from(f32::MAX) => {
            Some(value as f32)
        }
        Some(value) => {
            return Err(format!(
                "authored damping={value:?} must be finite, nonnegative and native-representable"
            ));
        }
    };
    let angular_damping = match read_authored_real(reader, sdf_path, PHYSX_ANGULAR_DAMPING)
        .map_err(|()| format!("{} has an unreadable authored value", PHYSX_ANGULAR_DAMPING))?
    {
        None => None,
        Some(value) if value.is_finite() && value >= 0.0 && value <= f64::from(f32::MAX) => {
            Some(value as f32)
        }
        Some(value) => {
            return Err(format!(
                "authored damping={value:?} must be finite, nonnegative and native-representable"
            ));
        }
    };
    let authored_linear = match read_authored_vec3(reader, sdf_path, ptok::PHYSICS_VELOCITY)
        .map_err(|()| {
            format!(
                "{} has an unreadable authored value",
                ptok::PHYSICS_VELOCITY
            )
        })? {
        Some(vel) => {
            let vel = local_vector_to_world(reader, sdf_path, conv.point_d(vel))
                .map_err(|error| error.to_string())?;
            if !lunco_physics::avian_backend_vector_is_valid(vel) {
                return Err("authored velocity exceeds the finite native vector range".to_owned());
            }
            Some(vel)
        }
        None => None,
    };
    let authored_angular = match read_authored_vec3(
        reader,
        sdf_path,
        ptok::PHYSICS_ANGULAR_VELOCITY,
    )
    .map_err(|()| {
        format!(
            "{} has an unreadable authored value",
            ptok::PHYSICS_ANGULAR_VELOCITY
        )
    })? {
        Some(ang) => {
            let ang = local_vector_to_world(
                reader,
                sdf_path,
                conv.dir_d(ang) * std::f64::consts::PI / 180.0,
            )
            .map_err(|error| error.to_string())?;
            if !lunco_physics::avian_backend_vector_is_valid(ang) {
                return Err("authored velocity exceeds the finite native vector range".to_owned());
            }
            Some(ang)
        }
        None => None,
    };
    let material = read_physics_material(reader, sdf_path)
        .map_err(|error| format!("physics material: {error}"))?;
    // Seed all three properties before RigidBody's OnAdd observer. A partial
    // override can otherwise rescale zero/invalid native geometry on insertion.
    commands.entity(entity).try_insert((
        native.0,
        native.1,
        native.2,
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
        overrides,
    ));
    if let Some(d) = linear_damping {
        commands.entity(entity).try_insert(LinearDamping(d));
    }
    if let Some(d) = angular_damping {
        commands.entity(entity).try_insert(AngularDamping(d));
    }
    insert_physics_material(commands, entity, material);
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
    Ok((
        has_volume && overrides.mass.is_none(),
        has_volume && overrides.principal.is_none(),
        has_volume && overrides.center.is_none(),
    ))
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
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
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
    let material = read_physics_material(reader, sdf_path).map_err(|_| ())?;
    insert_physics_material(commands, entity, material);
    Ok(())
}

/// Realize already validated material values before the next native physics step.
pub fn insert_physics_material(
    commands: &mut Commands,
    entity: Entity,
    material: Option<material::PhysicsMaterial>,
) {
    if let Some(pm) = material {
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

            // Arm material contact modification before collider admission so
            // the native pair contract includes authored static friction.
            if (friction.static_coefficient - friction.dynamic_coefficient).abs() > f32::EPSILON {
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

    use super::{ColliderBuildOutcome, ColliderProjectionError, build_collider_from_usd};
    use bevy::math::Vec3;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;
    use openusd_schemas::physics::CollisionApprox;

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
    fn authored_static_trimeshes_correct_internal_edges() {
        use avian3d::prelude::TrimeshFlags;

        for (source, path) in [
            (MESH_FIXTURE, "/Tri"),
            (super::extract_parity_tests::FLAT_TERRAIN_FIXTURE, "/Ground"),
        ] {
            let stage = stage_from_source(source);
            let ColliderBuildOutcome::Built(collider) =
                build_collider_from_usd(&stage.view(), &SdfPath::new(path).unwrap())
                    .expect("valid static collision geometry")
            else {
                panic!("static geometry must produce a collider");
            };
            let mesh = collider
                .shape_scaled()
                .as_trimesh()
                .expect("exact triangle mesh");
            assert!(
                mesh.flags()
                    .contains(TrimeshFlags::FIX_INTERNAL_EDGES.into()),
                "authored static meshes must suppress ghost contacts at internal triangle edges"
            );
        }
    }

    #[test]
    fn mesh_collision_approximation_selects_convex_hull() {
        let stage = stage_from_source(MESH_FIXTURE);
        let view = stage.view();

        let ColliderBuildOutcome::Built(trimesh) =
            build_collider_from_usd(&view, &SdfPath::new("/Tri").unwrap())
                .expect("valid transform")
        else {
            panic!("default mesh → trimesh collider");
        };
        let ColliderBuildOutcome::Built(hull) =
            build_collider_from_usd(&view, &SdfPath::new("/Hull").unwrap())
                .expect("valid transform")
        else {
            panic!("convexHull approximation → collider");
        };
        assert_ne!(
            format!("{trimesh:?}"),
            format!("{hull:?}"),
            "`physics:approximation = convexHull` must build a DIFFERENT collider than the default trimesh"
        );
        assert!(build_collider_from_usd(&view, &SdfPath::new("/BadHull").unwrap()).is_err());
        assert!(matches!(
            build_collider_from_usd(&view, &SdfPath::new("/BoundingCube").unwrap()),
            Err(ColliderProjectionError::UnsupportedApproximation {
                approximation: CollisionApprox::BoundingCube,
                ..
            })
        ));
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
    double3 xformOp:scale:wide = (1e100, 1, 1)
    uniform token[] xformOpOrder = ["xformOp:scale:wide"]
}
"#;
        let stage = stage_from_source(SOURCE);
        let view = stage.view();

        let ColliderBuildOutcome::Built(scaled) =
            build_collider_from_usd(&view, &SdfPath::new("/Scaled").unwrap())
                .expect("named scale is a valid composed transform")
        else {
            panic!("scaled cube collider");
        };
        assert_eq!(scaled.scale(), Vec3::new(2.0, 3.0, 4.0));

        let error = build_collider_from_usd(&view, &SdfPath::new("/Malformed").unwrap())
            .expect_err("a scale outside the native collider representation must be rejected");
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
    use lunco_usd_bevy_stage::StageView;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    fn stage_from_source(source: &str) -> CanonicalStage {
        CanonicalStage::from_recipe(&StageRecipe::from_source("test.usda", source))
            .expect("compose in-memory USDA fixture")
    }

    // A rover chassis (RigidBodyAPI, mass 500) with a child Cube collider
    // (CollisionAPI) offset by an authored xformOp:translate — the compound path.
    const FIXTURE: &str = "#usda 1.0\n\ndef Xform \"Rover\" (\n    prepend apiSchemas = [\"PhysicsRigidBodyAPI\"]\n)\n{\n    double physics:mass = 500\n    def Cube \"Body\" (\n        prepend apiSchemas = [\"PhysicsCollisionAPI\"]\n    )\n    {\n        double size = 2\n        double3 xformOp:translate = (0, 1, 0)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";

    pub(super) const FLAT_TERRAIN_FIXTURE: &str = "#usda 1.0\n\ndef Plane \"Ground\" (\n    prepend apiSchemas = [\"PhysicsCollisionAPI\", \"LunCoTerrainAPI\"]\n)\n{\n    double width = 100\n    double length = 100\n    token axis = \"Y\"\n    bool physics:collisionEnabled = true\n}\n";

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
    ) -> (Option<RigidBody>, Option<String>, Option<f32>, bool, bool) {
        let mut world = World::new();
        let e = world
            .spawn((
                lunco_physics::PhysicsOrderKey(path.to_string()),
                Transform::IDENTITY,
                GlobalTransform::default(),
            ))
            .id();
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
            lunco_physics::collider_entities(&world, e)
                .next()
                .and_then(|collider| world.get::<Collider>(collider))
                .map(|c| format!("{c:?}")),
            world.get::<Mass>(e).map(|m| m.0),
            world.get::<super::ShouldBeDynamic>(e).is_some(),
            world
                .get::<SweptCcd>(e)
                .is_some_and(|ccd| !ccd.filter.is_empty()),
        )
    }

    #[test]
    fn extract_avian_from_stageview_builds_full_dynamic_body() {
        let stage = stage_from_source(FIXTURE);
        let view = stage.view();
        let rover = SdfPath::new("/Rover").unwrap();

        let live = run_extract(&view, &rover);

        // The LIVE path actually produced a full dynamic body: Kinematic
        // (settling to Dynamic via ShouldBeDynamic) + attached colliders + mass.
        assert_eq!(live.0, Some(RigidBody::Kinematic), "live: rigid body");
        assert!(
            live.1.is_some(),
            "live: attached colliders built off the stage"
        );
        assert_eq!(
            live.2,
            Some(500.0),
            "live: authored mass read off the stage"
        );
        assert!(live.3, "live: ShouldBeDynamic (settles to Dynamic)");
    }

    #[test]
    fn authored_ccd_is_projected_only_for_enabled_dynamic_bodies() {
        for (attrs, enabled, admitted) in [
            ("", false, true),
            ("bool physxRigidBody:enableCCD = true", true, true),
            ("bool physxRigidBody:enableCCD = false", false, true),
            (
                "bool physxRigidBody:enableCCD = true\n bool physics:rigidBodyEnabled = false",
                false,
                true,
            ),
            (
                "bool physxRigidBody:enableCCD = true\n bool physics:kinematicEnabled = true",
                false,
                true,
            ),
            ("string physxRigidBody:enableCCD = \"true\"", false, false),
        ] {
            let source = FIXTURE.replacen(
                "double physics:mass = 500",
                &format!("{attrs}\n double physics:mass = 500"),
                1,
            );
            let stage = stage_from_source(&source);
            let facts = run_extract(&stage.view(), &SdfPath::new("/Rover").unwrap());
            assert_eq!(facts.4, enabled, "{attrs}");
            assert_eq!(facts.0.is_some(), admitted, "{attrs}");
        }
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
    fn omitted_mass_retains_checked_geometry_ownership() {
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
            live.2,
            Some(0.008),
            "the two-centimetre cube must retain its volume-times-standard-density mass"
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
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
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
def Xform "Wheel" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysxVehicleWheelAPI"] ) {}
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
    fn angular_drives_share_units_without_scaling_acceleration_gains_by_length() {
        for (drive_type, scale, expected_gain_scale) in [
            ("force", 1.0, 1.0),
            ("force", 0.01, 0.0001),
            ("acceleration", 1.0, 1.0),
            ("acceleration", 0.01, 1.0),
        ] {
            let source = format!(
                r#"#usda 1.0
( metersPerUnit = {scale} )
def Xform "Host" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {{}}
def Xform "Link" ( prepend apiSchemas = ["PhysicsRigidBodyAPI"] ) {{}}
def PhysicsRevoluteJoint "Hinge" ( prepend apiSchemas = ["PhysicsDriveAPI:angular"] ) {{
    rel physics:body0 = </Host>
    rel physics:body1 = </Link>
    uniform token drive:angular:physics:type = "{drive_type}"
    float drive:angular:physics:targetPosition = 90
    float drive:angular:physics:targetVelocity = 180
    float drive:angular:physics:stiffness = 300
    float drive:angular:physics:damping = 30
    float drive:angular:physics:maxForce = 100
}}
"#
            );
            let stage = stage_from_source(&source);
            let path = SdfPath::new("/Hinge").unwrap();
            let joint = read_joint_spec(&stage.view(), &path).expect("standard angular drive");
            let joint_drive = joint.drive.unwrap();
            // Gear relations enter exactly the same schema reader.
            let gear_drive =
                lunco_usd_avian_reader::joint::read_drive(&stage.view(), &path, "angular")
                    .unwrap()
                    .unwrap();
            for drive in [joint_drive, gear_drive] {
                assert_eq!(drive.target_position, Some(std::f64::consts::FRAC_PI_2));
                assert_eq!(drive.target_velocity, Some(std::f64::consts::PI));
                assert!((drive.max_force.unwrap() - 100.0 * scale * scale).abs() < 1e-12);
                let expected_k = 300.0f64.to_degrees() * expected_gain_scale;
                let expected_d = 30.0f64.to_degrees() * expected_gain_scale;
                match drive.motor_model().unwrap() {
                    MotorModel::ForceBased { stiffness, damping }
                    | MotorModel::AccelerationBased { stiffness, damping } => {
                        assert_eq!(stiffness, expected_k as f32);
                        assert_eq!(damping, expected_d as f32);
                    }
                    other => panic!("unexpected model {other:?}"),
                }
            }
        }
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
        let stage = lunco_usd_bevy_stage::canonical::CanonicalStage::from_recipe(
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

    /// Canonical joint axes and anchors use the stage's declared convention.
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

        // Stage +Z is canonical +Y.
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

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-5
    }

    fn write_and_compose(_name: &str, body: &str) -> CanonicalStage {
        stage_from_source(body)
    }

    #[test]
    fn joint_anchor_defaults_are_independent_of_body_placement_and_world_endpoint() {
        let source = r#"#usda 1.0
(
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Body" (prepend apiSchemas = ["PhysicsRigidBodyAPI"])
{
    double3 xformOp:translate = (100000001, 2, 0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
}
def PhysicsFixedJoint "Joint"
{
    rel physics:body0 = </Body>
    point3f physics:localPos0 = (1, 2, 3)
}
"#;
        let stage = stage_from_source(source);
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Joint").unwrap()).unwrap();
        assert_eq!(joint.body0_path, "/Body");
        assert_eq!(joint.body1_path, "");
        assert_eq!(joint.local_pos0, DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(joint.local_pos1, DVec3::ZERO);
        assert_eq!(joint.local_rot1, bevy::math::DQuat::IDENTITY);
        let stage = stage_from_source(&source.replace("point3f physics:localPos0 = (1, 2, 3)", ""));
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Joint").unwrap()).unwrap();
        assert_eq!(joint.local_pos0, DVec3::ZERO);
        assert_eq!(joint.local_pos1, DVec3::ZERO);
    }

    #[test]
    fn joint_endpoint_composition_bakes_scale_and_retains_descendant_pose() {
        let source = r#"#usda 1.0
(
    upAxis = "Z"
    metersPerUnit = 0.01
)
def Xform "Body" (prepend apiSchemas = ["PhysicsRigidBodyAPI"])
{
    double3 xformOp:translate = (10000000000, 0, 0)
    float3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
    def Xform "Socket"
    {
        double3 xformOp:translate = (100, 0, 0)
        float xformOp:rotateZ = 90
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateZ"]
    }
}
def Xform "Other" (prepend apiSchemas = ["PhysicsRigidBodyAPI"]) {}
def PhysicsFixedJoint "Joint"
{
    rel physics:body0 = </Body/Socket>
    rel physics:body1 = </Other>
    point3f physics:localPos0 = (100, 0, 0)
}
"#;
        let stage = stage_from_source(source);
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Joint").unwrap()).unwrap();
        assert_eq!(joint.body0_path, "/Body");
        assert!(close(joint.local_pos0, DVec3::new(2.0, 0.0, -2.0)));
        assert!(close(joint.local_rot0 * DVec3::X, -DVec3::Z));
        assert_eq!(joint.local_pos1, DVec3::ZERO);
        let source = source.replace("</Body/Socket>", "</Body>");
        let stage = stage_from_source(&source);
        let joint = read_joint_spec(&stage.view(), &SdfPath::new("/Joint").unwrap()).unwrap();
        assert!(close(joint.local_pos0, DVec3::new(2.0, 0.0, 0.0)));
    }

    /// A descendant target retains its frame when resolving to a host body.
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
            close(j.local_pos0, DVec3::new(0.0, 1.0, 0.0)),
            "the target origin must resolve in the host body's frame: {:?}",
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
}

#[cfg(test)]
mod collider_ownership_tests {
    use super::*;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
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
        double3 xformOp:translate = (1e100, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("bad.usda", source))
            .expect("build stage");
        let root = SdfPath::new("/Root").unwrap();
        let error = collect_body_colliders_from_usd(&stage.view(), &root)
            .expect_err("malformed authored transform must reject compound discovery");
        assert!(error.to_string().contains("/Root/Body"));
    }

    #[test]
    fn decomposed_mesh_retains_one_authored_collider_identity() {
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
        let parts = collect_body_colliders_from_usd(&view, &root)
            .expect("each authored collider may retain its own cooked composite shape");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].source_path, "/Root/Proxy");
        assert!(parts[0].collider.shape_scaled().as_compound().is_some());
        let (has_collider, body) = extract(&view, "/Root");
        assert!(has_collider);
        assert_eq!(body, Some(RigidBody::Kinematic));
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
        let error = collect_body_colliders_from_usd(&stage.view(), &root)
            .expect_err("an invalid collisionEnabled must reject incomplete compound geometry");
        assert!(
            matches!(error, ColliderProjectionError::Backend { prim, detail }
            if prim == "/Root/Body" && detail.contains("physics:collisionEnabled"))
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
    fn extract(
        view: &lunco_usd_bevy_stage::StageView<'_>,
        path: &str,
    ) -> (bool, Option<RigidBody>) {
        let mut world = World::new();
        let entity = world
            .spawn((
                lunco_physics::PhysicsOrderKey(path.to_owned()),
                Transform::IDENTITY,
                GlobalTransform::default(),
            ))
            .id();
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
            lunco_physics::collider_entities(&world, entity)
                .next()
                .is_some(),
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
        let pieces = collect_body_colliders_from_usd(&view, &leg).expect("valid transforms");
        assert_eq!(
            pieces.len(),
            1,
            "the leg's compound is its strut alone — the pad is its own body, not \
             the leg's geometry"
        );
    }

    #[test]
    fn inactive_geometry_is_excluded_from_live_and_prepared_compounds() {
        let recipe = StageRecipe::from_source(
            "inactive-collider.usda",
            r#"#usda 1.0
(
    defaultPrim = "Body"
    upAxis = "Y"
    metersPerUnit = 1
)
def Xform "Body" (prepend apiSchemas = "PhysicsRigidBodyAPI")
{
    def Cube "Hull" (prepend apiSchemas = "PhysicsCollisionAPI")
    {
        double size = 2
    }
    def Cube "DisabledPanel" (
        active = false
        prepend apiSchemas = "PhysicsCollisionAPI"
    )
    {
        double size = 20
        uniform token purpose = "proxy"
    }
    def Xform "DisabledAssembly" (active = false)
    {
        def Cube "Panel" (prepend apiSchemas = "PhysicsCollisionAPI")
        {
            double size = 30
        }
    }
}
"#,
        );
        let stage = CanonicalStage::from_recipe(&recipe).expect("compose inactive geometry");
        let asset = UsdStageAsset::from_recipe(recipe).expect("prepare inactive geometry");
        let root = SdfPath::new("/Body").unwrap();
        let live = stage.view();
        for reader in [
            &live as &dyn lunco_usd_bevy_stage::read::UsdReadObject,
            asset.projection_plan.as_ref() as &dyn lunco_usd_bevy_stage::read::UsdReadObject,
        ] {
            let shapes = collect_body_colliders_from_usd(reader, &root).unwrap();
            assert_eq!(
                shapes.len(),
                1,
                "inactive proxy must not suppress the live hull"
            );
            let bounds = shapes[0]
                .collider
                .aabb(shapes[0].position.0, shapes[0].rotation.0, 0.0);
            assert_eq!(bounds.min, Vec3::splat(-1.0));
            assert_eq!(bounds.max, Vec3::splat(1.0));
        }
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
        let pieces = collect_body_colliders_from_usd(&view, &hull).expect("valid transforms");
        assert_eq!(
            pieces.len(),
            1,
            "expected exactly the proxy — got {} pieces, so the render mesh or the \
             guide marker is being collided too",
            pieces.len()
        );
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
    /// body's native collider set; its visual entity receives neither component.
    #[test]
    fn collider_under_rigid_body_ancestor_stays_a_compound_piece() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let (has_collider, body) = extract(&cs.view(), "/Mission/CompoundLander/Hull");
        assert!(
            !has_collider,
            "a visual child must not duplicate its native collider"
        );
        assert_eq!(body, None, "nor its own rigid body");
    }

    /// A collision-enabled rigid-body root contributes its own geometry.
    #[test]
    fn rigid_body_root_without_collider_children_uses_its_own_shape() {
        let recipe = StageRecipe::from_source("t.usda", SCENE);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let lander = SdfPath::new("/Mission/BareLander").unwrap();
        let view = cs.view();
        let parts = collect_body_colliders_from_usd(&view, &lander).expect("valid transforms");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].source_path, "/Mission/BareLander");
        assert_eq!(parts[0].position.0, DVec3::ZERO);
        let (has_collider, _) = extract(&view, "/Mission/BareLander");
        assert!(
            has_collider,
            "a bare rigid-body root must collide via its own shape"
        );
    }

    #[test]
    fn body_root_primitive_axes_match_descendant_collider_frames() {
        for up_axis in ["Y", "Z"] {
            for axis in ["X", "Y", "Z"] {
                for shape in ["Cylinder", "Cone", "Capsule", "Plane"] {
                    let source = format!(
                        r#"#usda 1.0
(
    upAxis = "{up_axis}"
    metersPerUnit = 1.0
)
def {shape} "Body" (prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"])
{{
    uniform token axis = "{axis}"
    double radius = 0.34
    double height = 0.25
    double width = 0.68
    double length = 0.25
    def {shape} "Child" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {{
        uniform token axis = "{axis}"
        double radius = 0.34
        double height = 0.25
        double width = 0.68
        double length = 0.25
    }}
}}
"#
                    );
                    let recipe = StageRecipe::from_source("axes.usda", &source);
                    let stage = CanonicalStage::from_recipe(&recipe).unwrap();
                    let view = stage.view();
                    let root = SdfPath::new("/Body").unwrap();
                    let parts = collect_body_colliders_from_usd(&view, &root).unwrap();
                    assert_eq!(parts.len(), 2);
                    assert!(
                        parts[0].rotation.0.abs_diff_eq(parts[1].rotation.0, 1e-6),
                        "{up_axis}-up {axis}-axis {shape}: root and descendant geometry must share a basis"
                    );
                    let expected =
                        lunco_usd_bevy_scene::read_geometry_orientation(&view, &root).unwrap();
                    assert!(parts[0].rotation.0.as_dquat().abs_diff_eq(expected, 1e-6));
                }
            }
        }
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
            "the visual collider prim must not acquire an independent body"
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
    def Cube "BaseCap" (prepend apiSchemas = ["PhysicsCollisionAPI"])
    {
        double size = 1.0
        double3 xformOp:translate = (0, -2, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;
        let recipe = StageRecipe::new(
            root_id.clone(),
            HashMap::from([
                (root_id, scene.as_bytes().to_vec()),
                (child_id, child.as_bytes().to_vec()),
            ]),
        );

        let live = CanonicalStage::from_recipe(&recipe).expect("compose referenced body");
        let assembly = SdfPath::new("/Scene/Assembly").unwrap();
        let live_view = live.view();
        assert!(live_view.has_prim(&SdfPath::new("/Scene/Assembly/EndCap").unwrap()));
        let live_parts = collect_body_colliders_from_usd(&live_view, &assembly)
            .expect("live composed reference has valid collision transforms");
        assert_eq!(
            live_parts
                .iter()
                .map(|part| part.source_path.as_str())
                .collect::<Vec<_>>(),
            [
                "/Scene/Assembly",
                "/Scene/Assembly/BaseCap",
                "/Scene/Assembly/EndCap"
            ]
        );
        let live_shapes = &live_parts;
        assert_eq!(
            live_shapes.len(),
            3,
            "live composition must keep root and child shapes"
        );
        assert_eq!(live_shapes[0].position.0, DVec3::ZERO);
        assert_eq!(live_shapes[1].position.0, DVec3::new(0.0, -2.0, 0.0));
        assert_eq!(live_shapes[2].position.0, DVec3::new(0.0, 2.0, 0.0));

        let prepared =
            UsdStageAsset::from_recipe(recipe).expect("prepare composed referenced body");
        let prepared_parts = collect_body_colliders_from_usd(&*prepared.projection_plan, &assembly)
            .expect("prepared reference has valid collision transforms");
        assert_eq!(
            prepared_parts
                .iter()
                .map(|part| &part.source_path)
                .collect::<Vec<_>>(),
            live_parts
                .iter()
                .map(|part| &part.source_path)
                .collect::<Vec<_>>()
        );
        let prepared_shapes = &prepared_parts;
        assert_eq!(
            prepared_shapes.len(),
            3,
            "prepared composition must keep root and child shapes"
        );
        assert_eq!(prepared_shapes[0].position.0, DVec3::ZERO);
        assert_eq!(prepared_shapes[1].position.0, DVec3::new(0.0, -2.0, 0.0));
        assert_eq!(prepared_shapes[2].position.0, DVec3::new(0.0, 2.0, 0.0));
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

#[cfg(test)]
mod stage_projection_reset_tests {
    use super::{prepare_stage_projection_reset, subtree_requires_stage_projection_reset};
    use avian3d::prelude::RigidBody;
    use bevy::prelude::{Handle, World};
    use lunco_usd_bevy_scene::UsdPrimPath;
    use lunco_usd_bevy_stage::UsdStageAsset;

    #[test]
    fn local_refresh_promotes_physics_subtrees_and_keeps_visual_subtrees_local() {
        let mut world = World::new();
        let stage = Handle::<UsdStageAsset>::default();
        world.spawn((
            UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/Scene/Rover/Chassis".into(),
            },
            RigidBody::Dynamic,
        ));
        world.spawn(UsdPrimPath {
            stage_handle: stage.clone(),
            path: "/Scene/Camera/Visual".into(),
        });

        assert!(subtree_requires_stage_projection_reset(
            &world,
            stage.id(),
            "/Scene/Rover"
        ));
        assert!(subtree_requires_stage_projection_reset(
            &world,
            stage.id(),
            "/Scene"
        ));
        assert!(!subtree_requires_stage_projection_reset(
            &world,
            stage.id(),
            "/Scene/Camera"
        ));
    }

    #[test]
    fn reset_reports_missing_avian_graph_resources_without_mutating_the_stage() {
        let mut world = World::new();
        let stage = Handle::<UsdStageAsset>::default();
        let body = world
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Scene/Rover/Chassis".into(),
                },
                RigidBody::Dynamic,
            ))
            .id();

        let error = prepare_stage_projection_reset(&mut world, stage.id())
            .expect_err("physics reset requires Avian's graph resources");

        assert!(error.contains("Avian island, joint, and contact graph resources"));
        assert_eq!(world.get::<RigidBody>(body), Some(&RigidBody::Dynamic));
    }
}
