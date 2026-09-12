//! Render-free USD scene contracts for Bevy.
//!
//! This package owns the ECS identity and lifecycle facts shared by USD
//! projection consumers. It intentionally contains no meshes, materials,
//! cameras, lights, windows, or render queue. The visual adapter inserts
//! [`UsdSceneProjected`] after it has committed the structural USD hierarchy;
//! physics, simulation, tools, and UI can consume that fact without importing
//! the visual implementation.

mod geometry;

use bevy::asset::{AssetEvent, AssetLoadFailedEvent};
use bevy::ecs::hierarchy::ChildOf;
use bevy::prelude::*;
use lunco_usd_bevy_core::{UsdInstanceProjection, UsdInstanceRoot, UsdStageAsset};

pub use geometry::{
    read_primitive_axis, read_shape_dims, read_usd_mesh_indexed, read_usd_mesh_points,
    read_usd_mesh_topology, usd_axis_to_quat, ShapeDims, UsdMeshTopology,
};

/// Installs render-free USD scene lifecycle bookkeeping.
///
/// The scene contract owns stage-load failure cleanup and revision invalidation;
/// visual, physics, and simulation projections consume the resulting facts.
pub struct UsdScenePlugin;

impl Plugin for UsdScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdStageRevision>()
            .add_systems(PreUpdate, bump_usd_stage_revision)
            .add_systems(
                Update,
                fail_awaiting_stage_prims.run_if(
                    bevy::ecs::schedule::common_conditions::on_message::<
                        AssetLoadFailedEvent<UsdStageAsset>,
                    >,
                ),
            );
    }
}

/// The USD prim represented by a Bevy entity.
///
/// This is scene identity, not a visual component. Every domain projection
/// that needs to associate an ECS entity with composed USD reads uses this
/// component and the stage asset handle it carries.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    /// Handle to the loaded composed USD stage asset.
    pub stage_handle: Handle<UsdStageAsset>,
    /// Absolute USD prim path within the stage.
    pub path: String,
}

impl Default for UsdPrimPath {
    fn default() -> Self {
        Self {
            stage_handle: Handle::default(),
            path: "/".to_owned(),
        }
    }
}

/// Monotonic signal that a USD-to-ECS scene projection may have changed.
///
/// Consumers use this resource as an invalidation signal for derived indexes;
/// it is deliberately a counter rather than a hash of derived state.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsdStageRevision(pub u64);

impl UsdStageRevision {
    /// Publish one scene projection change.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Raise [`UsdStageRevision`] when scene identity or the loaded stage changes.
pub fn bump_usd_stage_revision(
    mut revision: ResMut<UsdStageRevision>,
    added: Query<(), Added<UsdPrimPath>>,
    mut removed: RemovedComponents<UsdPrimPath>,
    mut stage_events: MessageReader<AssetEvent<UsdStageAsset>>,
) {
    let stage_changed = stage_events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Modified { .. } | AssetEvent::LoadedWithDependencies { .. }
        )
    });
    // Always drain removals so a later frame cannot re-deliver an old removal.
    let any_removed = removed.read().next().is_some();
    if !added.is_empty() || any_removed || stage_changed {
        revision.bump();
    }
}

/// Marks the entity whose structural USD projection has been committed.
///
/// The name is intentionally independent of presentation: physics and
/// simulation may consume the committed USD hierarchy even while a visual mesh
/// is still streaming or no renderer is present.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneProjected;

/// Boundary after the USD asset has been synchronized into the live ECS scene.
///
/// The visual projector owns the producer system, while headless projections
/// use this set for ordering without depending on the visual implementation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsdSceneSyncSet;

/// Boundary after bounded USD scene and geometry projection has committed.
///
/// Simulation, shader intent, and presentation readiness consume this shared
/// contract instead of naming the visual projector's systems directly.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsdVisualProjectionSet;

/// Render target selected by a simulation-side visual split.
///
/// A physical owner may retain the USD identity while its mesh is moved to a
/// child with the presentation transform. The target remains render-free so
/// shader and simulation projections do not depend on the visual crate.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualMeshTarget(pub Entity);

/// Marks a render target whose appearance is owned by a custom shader.
///
/// This is an intent marker only. The render binder consumes it together with
/// `ShaderLook`; the scene contract must not depend on `bevy_pbr`.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdVisualShaderBound;

/// Marks a scene prim whose stage has not finished loading.
///
/// This is a scene-lifecycle fact rather than a visual implementation detail:
/// camera presentation, readiness, tools, and simulation use it to distinguish
/// an incomplete scene from one that has no authored content.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneAwaitingStage;

/// Marks a scene prim admitted to the bounded structural projection queue.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneProjectionQueued;

/// Marks a scene prim whose detached geometry projection is still running.
/// Structural scene projection is complete, but presentation readiness may
/// still wait for the geometry and its bounds.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneGeometryPending;

/// Terminal failure produced while projecting a composed USD prim.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
pub struct UsdSceneProjectionFailed(pub String);

/// Terminal asset failure recorded until the scene transaction consumes it.
///
/// This is a scene-lifecycle fact, not a visual diagnostic. Keeping it in the
/// render-free scene package lets headless simulation and document commands
/// close a failed mount without depending on placeholder presentation.
#[derive(Resource, Debug, Clone)]
pub struct FailedSceneLoad {
    pub stage_id: bevy::asset::AssetId<UsdStageAsset>,
    pub error: String,
}

/// Makes a failed stage load terminal for prims parked on it.
fn fail_awaiting_stage_prims(
    mut ev: MessageReader<AssetLoadFailedEvent<UsdStageAsset>>,
    q: Query<(Entity, &UsdPrimPath), With<UsdSceneAwaitingStage>>,
    mut commands: Commands,
) {
    for failure in ev.read() {
        let parked: Vec<Entity> = q
            .iter()
            .filter(|(_, prim_path)| prim_path.stage_handle.id() == failure.id)
            .map(|(entity, _)| entity)
            .collect();
        if parked.is_empty() {
            continue;
        }
        error!(
            "[usd-scene] stage {} failed to load ({}) — {} prim(s) waiting on it will never instantiate and are being dropped",
            failure.path,
            failure.error,
            parked.len()
        );
        for entity in parked {
            commands.entity(entity).try_despawn();
        }
        commands.insert_resource(FailedSceneLoad {
            stage_id: failure.id,
            error: failure.error.to_string(),
        });
    }
}

/// Marks a USD prim that owns a primitive fallback while a glTF scene payload
/// is loading. The visual projector inserts this scene fact; the optional
/// diagnostics plugin consumes it to hide or replace the fallback.
#[derive(Component, Debug, Clone, Copy)]
pub struct GlbPlaceholder;

/// Asset URI associated with a [`GlbPlaceholder`].
#[derive(Component, Debug, Clone)]
pub struct PlaceholderAssetUri(pub String);

/// Marks an entity whose USD xform or appearance contains time-sampled data.
///
/// The visual adapter plans and samples the channels, while physics and other
/// consumers can use the marker to distinguish authored animation from static
/// scene state without depending on the visual crate.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct UsdAnimated;

/// Root of a live, scene-owned USD mount.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneRoot;

/// Root marker for a render-only USD preview hierarchy.
#[derive(Component, Default, Debug, Clone, Copy)]
pub struct UsdPreviewOnly;

/// Returns whether an entity belongs to a preview hierarchy.
pub fn is_preview_only(
    entity: Entity,
    child_of: &Query<&ChildOf>,
    preview_roots: &Query<(), With<UsdPreviewOnly>>,
) -> bool {
    let mut current = entity;
    for _ in 0..1024 {
        if preview_roots.contains(current) {
            return true;
        }
        let Ok(parent) = child_of.get(current) else {
            return false;
        };
        current = parent.parent();
    }
    warn!(
        "[usd-scene] preview hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    false
}

/// World-query variant of [`is_preview_only`].
pub fn is_preview_only_entity(world: &World, entity: Entity) -> bool {
    let mut current = entity;
    for _ in 0..1024 {
        if world.get::<UsdPreviewOnly>(current).is_some() {
            return true;
        }
        let Some(parent) = world.get::<ChildOf>(current).map(ChildOf::parent) else {
            return false;
        };
        current = parent;
    }
    warn!(
        "[usd-scene] preview hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    false
}

/// Resolve the live scene root above an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneRootAncestorError {
    /// A parent relationship points to an entity that no longer exists.
    MissingParentEntity,
    /// The hierarchy exceeded the bounded traversal depth.
    DepthExceeded,
}

pub fn scene_root_ancestor(
    entity: Entity,
    scene_roots: &Query<(), With<UsdSceneRoot>>,
    child_of: &Query<&ChildOf>,
    entities: &Query<Entity>,
) -> Result<Option<Entity>, SceneRootAncestorError> {
    let mut current = entity;
    for _ in 0..1024 {
        if scene_roots.contains(current) {
            return Ok(Some(current));
        }
        let Ok(parent) = child_of.get(current) else {
            return Ok(None);
        };
        current = parent.parent();
        if !entities.contains(current) {
            return Err(SceneRootAncestorError::MissingParentEntity);
        }
    }
    warn!(
        "[usd-scene] scene hierarchy exceeded 1024 ancestors at {:?}",
        entity
    );
    Err(SceneRootAncestorError::DepthExceeded)
}

/// Resolve the instance namespace for an ECS entity.
pub fn instance_key(
    entity: Entity,
    provenance: &Query<&lunco_core::Provenance>,
    global_ids: &Query<&lunco_core::GlobalEntityId>,
    instance_roots: &Query<(), With<UsdInstanceRoot>>,
    projections: &Query<&UsdInstanceProjection>,
) -> Option<u64> {
    instance_key_from_projection(
        entity,
        provenance,
        global_ids,
        instance_roots,
        projections.get(entity).ok(),
    )
}

/// Resolve instance scope when the caller already fetched the projection.
pub fn instance_key_from_projection(
    entity: Entity,
    provenance: &Query<&lunco_core::Provenance>,
    global_ids: &Query<&lunco_core::GlobalEntityId>,
    instance_roots: &Query<(), With<UsdInstanceRoot>>,
    projection: Option<&UsdInstanceProjection>,
) -> Option<u64> {
    match provenance.get(entity) {
        Ok(lunco_core::Provenance::Derived { parent, .. }) => Some(*parent),
        _ => projection
            .and_then(|projection| projection.root)
            .and_then(|root| global_ids.get(root).map(|id| id.get()).ok())
            .or_else(|| {
                instance_roots
                    .contains(entity)
                    .then(|| global_ids.get(entity).map(|id| id.get()).ok())
                    .flatten()
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    #[test]
    fn scene_root_ancestor_finds_root_and_rejects_missing_parent() {
        let mut world = World::new();
        let root = world.spawn(UsdSceneRoot).id();
        let child = world.spawn(ChildOf(root)).id();
        let detached = world.spawn_empty().id();
        let missing_parent = world
            .spawn(ChildOf(Entity::from_raw_u32(999_999).unwrap()))
            .id();
        let mut state: SystemState<(
            Query<(), With<UsdSceneRoot>>,
            Query<&ChildOf>,
            Query<Entity>,
        )> = SystemState::new(&mut world);
        let (scene_roots, child_of, entities) = state.get(&mut world).unwrap();

        assert_eq!(
            scene_root_ancestor(child, &scene_roots, &child_of, &entities,),
            Ok(Some(root))
        );
        assert_eq!(
            scene_root_ancestor(detached, &scene_roots, &child_of, &entities,),
            Ok(None)
        );
        assert_eq!(
            scene_root_ancestor(missing_parent, &scene_roots, &child_of, &entities,),
            Err(SceneRootAncestorError::MissingParentEntity)
        );
    }

    #[test]
    fn preview_state_walks_ancestors_but_not_detached_entities() {
        let mut world = World::new();
        let root = world.spawn(UsdPreviewOnly).id();
        let child = world.spawn(ChildOf(root)).id();
        let detached = world.spawn_empty().id();
        let mut state: SystemState<(Query<&ChildOf>, Query<(), With<UsdPreviewOnly>>)> =
            SystemState::new(&mut world);
        let (child_of, preview_roots) = state.get(&mut world).unwrap();

        assert!(is_preview_only(child, &child_of, &preview_roots));
        assert!(!is_preview_only(detached, &child_of, &preview_roots));
        assert!(is_preview_only_entity(&world, child));
        assert!(!is_preview_only_entity(&world, detached));
    }
}

#[cfg(test)]
mod stage_failure_tests {
    use super::*;
    use bevy::asset::{AssetLoadError, AssetLoadFailedEvent, AssetPath};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(UsdScenePlugin)
            .add_message::<AssetEvent<UsdStageAsset>>()
            .add_message::<AssetLoadFailedEvent<UsdStageAsset>>();
        app
    }

    fn parked(app: &mut App, handle: Handle<UsdStageAsset>) -> Entity {
        app.world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: handle,
                    path: "/Scene".into(),
                },
                UsdSceneAwaitingStage,
            ))
            .id()
    }

    fn fail(app: &mut App, handle: &Handle<UsdStageAsset>, path: &str) {
        app.world_mut()
            .resource_mut::<Messages<AssetLoadFailedEvent<UsdStageAsset>>>()
            .write(AssetLoadFailedEvent {
                id: handle.id(),
                path: AssetPath::from(path.to_string()),
                error: AssetLoadError::EmptyPath(AssetPath::from(path.to_string())),
            });
    }

    #[test]
    fn a_failed_stage_drops_the_prims_parked_on_it() {
        let mut app = app();
        let handle = Handle::<UsdStageAsset>::default();
        let entity = parked(&mut app, handle.clone());

        app.update();
        assert!(app.world().get_entity(entity).is_ok());

        fail(&mut app, &handle, "missing.usda");
        app.update();
        assert!(app.world().get_entity(entity).is_err());
        assert!(app.world().contains_resource::<FailedSceneLoad>());
    }

    #[test]
    fn a_different_stages_failure_leaves_this_prim_waiting() {
        let mut app = app();
        let mine = Handle::<UsdStageAsset>::default();
        let entity = parked(&mut app, mine);

        let other: Handle<UsdStageAsset> =
            bevy::asset::uuid_handle!("5ce7e000-0000-4000-8000-000000000001");
        fail(&mut app, &other, "someone_elses.usda");
        app.update();

        assert!(app.world().get_entity(entity).is_ok());
        assert!(!app.world().contains_resource::<FailedSceneLoad>());
    }
}
