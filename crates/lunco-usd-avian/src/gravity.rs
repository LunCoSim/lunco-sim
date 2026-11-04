//! Composed PhysicsScene gravity admission and notice-driven refresh.

use super::*;
use lunco_usd_bevy_scene::{UsdSceneChangeBatch, scene_root_ancestor};
use lunco_usd_bevy_stage::canonical::CanonicalStages;

#[derive(Component)]
pub(super) struct UsdPhysicsSceneSettings;

#[derive(Clone, Debug, PartialEq, Eq)]
struct GravitySourceLease {
    entity: Entity,
    stage: AssetId<UsdStageAsset>,
    path: String,
    root: Option<Entity>,
}

impl GravitySourceLease {
    fn is_live(&self, world: &World) -> bool {
        if world.get::<UsdPhysicsSceneSettings>(self.entity).is_none()
            || world
                .get::<UsdPrimPath>(self.entity)
                .is_none_or(|prim| prim.stage_handle.id() != self.stage || prim.path != self.path)
            || lunco_usd_bevy_scene::is_preview_only_entity(world, self.entity)
            || self.root.is_some_and(|root| {
                world.get::<UsdSceneRoot>(root).is_none()
                    || world
                        .get_resource::<lunco_core::SceneMountState>()
                        .is_some_and(|mount| !mount.contains_root(root))
            })
        {
            return false;
        }
        let Some(mut roots) =
            bevy::ecs::query::QueryState::<(), With<UsdSceneRoot>>::try_new(world)
        else {
            return false;
        };
        let Some(mut parents) = bevy::ecs::query::QueryState::<&ChildOf>::try_new(world) else {
            return false;
        };
        let Some(mut entities) = bevy::ecs::query::QueryState::<Entity>::try_new(world) else {
            return false;
        };
        if !scene_root_ancestor(
            self.entity,
            &roots.query(world),
            &parents.query(world),
            &entities.query(world),
        )
        .is_ok_and(|root| root == self.root)
        {
            return false;
        }
        let Some(asset) = world
            .get_resource::<Assets<UsdStageAsset>>()
            .and_then(|assets| assets.get(self.stage))
        else {
            return false;
        };
        let Some(canonical) = world.get_non_send::<CanonicalStages>() else {
            return false;
        };
        let (reader, _) = canonical.reader_for(self.stage, asset);
        let Ok(path) = SdfPath::new(&self.path) else {
            return false;
        };
        reader.type_name(&path).as_deref() == Some(ptok::PHYSICS_SCENE)
            && reader.is_active(&path).is_ok_and(|active| active)
    }
}

/// Source identity for the single admitted gravity fact; values remain owned
/// by PhysicsSceneGravity and the environment's Gravity resource.
#[derive(Resource, Default)]
pub(super) struct PhysicsSceneGravityOwner(Option<GravitySourceLease>);

impl lunco_core::SceneResource for PhysicsSceneGravityOwner {}

fn owner_root(
    entity: Entity,
    parents: &Query<&ChildOf>,
    previews: &Query<(), With<UsdPreviewOnly>>,
    roots: &Query<(), With<UsdSceneRoot>>,
    entities: &Query<Entity>,
    mount: Option<&lunco_core::SceneMountState>,
) -> Result<Option<Entity>, ()> {
    if is_preview_only(entity, parents, previews) {
        return Err(());
    }
    let root = scene_root_ancestor(entity, roots, parents, entities).map_err(|_| ())?;
    if root.is_some_and(|root| mount.is_some_and(|mount| !mount.contains_root(root))) {
        return Err(());
    }
    Ok(root)
}

fn queue_projection(
    entity: Entity,
    prim: &UsdPrimPath,
    root: Option<Entity>,
    stages: &Assets<UsdStageAsset>,
    canonical: &CanonicalStages,
    commands: &mut Commands,
) {
    let Some(asset) = stages.get(prim.stage_handle.id()) else {
        return;
    };
    let (reader, generation) = canonical.reader_for(prim.stage_handle.id(), asset);
    let Ok(path) = SdfPath::new(&prim.path) else {
        return;
    };
    if reader.type_name(&path).as_deref() != Some(ptok::PHYSICS_SCENE) {
        return;
    }
    let candidate = read_physics_scene_gravity(&reader, &path);
    let prim = prim.clone();
    commands.queue(move |world: &mut World| {
        if world.get::<UsdPhysicsSceneSettings>(entity).is_none()
            || world.get::<UsdPrimPath>(entity).is_none_or(|current| {
                current.path != prim.path || current.stage_handle != prim.stage_handle
            })
            || world.get_non_send::<CanonicalStages>().is_none_or(|stages| {
                stages.generation_for(prim.stage_handle.id()) != generation
            })
            || lunco_usd_bevy_scene::is_preview_only_entity(world, entity)
            || root.is_some_and(|root| world.get_resource::<lunco_core::SceneMountState>()
                .is_some_and(|mount| !mount.contains_root(root)))
        { return; }

        let source = GravitySourceLease { entity, stage: prim.stage_handle.id(), path: prim.path.clone(), root };
        if !source.is_live(world) { return; }
        let previous_owner = world.resource::<PhysicsSceneGravityOwner>().0.clone();
        let previous_is_live = previous_owner.as_ref().is_some_and(|owner| owner.is_live(world));
        let same_owner = previous_owner.as_ref() == Some(&source);
        let candidate = candidate.and_then(|(magnitude, direction)| {
            if let Some(existing) = world.get_resource::<lunco_environment::PhysicsSceneGravity>() {
                if previous_is_live && !same_owner
                    && (existing.magnitude != magnitude || existing.direction != direction)
                {
                    return Err(format!("PhysicsScene {source:?} disagrees with admitted source {previous_owner:?}: {} m/s² along {:?}; one physical world requires one consistent gravity setting", existing.magnitude, existing.direction));
                }
            }
            Ok((magnitude, direction))
        });
        let (magnitude, direction) = match candidate {
            Ok(candidate) => candidate,
            Err(detail) => {
                world.resource_scope(|world, mut faults: Mut<lunco_core::RuntimeFaults>| {
                    let mut holds = world.get_resource_or_insert_with(lunco_physics::PhysicsHolds::default);
                    report_physics_runtime_fault(Some(&mut faults), Some(&mut holds), entity,
                        prim.path.clone(), "usd-physics-gravity-invalid", detail.clone());
                });
                world.trigger(lunco_core::RuntimeError {
                    name: "usd-physics-gravity-invalid".into(), message: detail,
                });
                return;
            }
        };
        let previous = world.get_resource::<lunco_environment::PhysicsSceneGravity>();
        let unchanged = previous.is_some_and(|previous| previous.magnitude == magnitude && previous.direction == direction);
        if previous_is_live && unchanged {
            return;
        }
        // Celestial takeover owns Surface mode after site alignment. Updating
        // authored flat-world facts must not replace that selected model.
        let preserve_surface = previous_is_live
            && matches!(world.get_resource::<lunco_environment::Gravity>(), Some(lunco_environment::Gravity::Surface));
        world.resource_mut::<PhysicsSceneGravityOwner>().0 = Some(source);
        world.insert_resource(lunco_environment::PhysicsSceneGravity {
            prim: prim.path, magnitude, direction,
        });
        let next = lunco_environment::Gravity::flat(magnitude, direction);
        if !preserve_surface && world.get_resource::<lunco_environment::Gravity>() != Some(&next) {
            world.insert_resource(next);
        }
    });
}

pub(super) fn project_initial_gravity(
    trigger: On<Add<UsdPhysicsSceneSettings>>,
    owners: Query<&UsdPrimPath, With<UsdPhysicsSceneSettings>>,
    parents: Query<&ChildOf>,
    previews: Query<(), With<UsdPreviewOnly>>,
    roots: Query<(), With<UsdSceneRoot>>,
    entities: Query<Entity>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim) = owners.get(entity) else {
        return;
    };
    let Ok(root) = owner_root(
        entity,
        &parents,
        &previews,
        &roots,
        &entities,
        mount.as_deref(),
    ) else {
        return;
    };
    queue_projection(entity, prim, root, &stages, &canonical, &mut commands);
}

pub(super) fn refresh_gravity(
    mut notices: MessageReader<UsdSceneChangeBatch>,
    owners: Query<(Entity, &UsdPrimPath), With<UsdPhysicsSceneSettings>>,
    parents: Query<&ChildOf>,
    previews: Query<(), With<UsdPreviewOnly>>,
    roots: Query<(), With<UsdSceneRoot>>,
    entities: Query<Entity>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
) {
    let mut changed = std::collections::HashMap::<AssetId<UsdStageAsset>, Vec<String>>::new();
    for notice in notices.read() {
        changed.entry(notice.stage_id).or_default().extend(
            notice
                .resynced_prim_paths
                .iter()
                .chain(&notice.info_prim_paths)
                .cloned(),
        );
    }
    if changed.is_empty() {
        return;
    }
    let mut owners = owners.iter().collect::<Vec<_>>();
    owners.sort_by(|left, right| left.1.path.cmp(&right.1.path));
    for (entity, prim) in owners {
        let Some(paths) = changed.get(&prim.stage_handle.id()) else {
            continue;
        };
        if !paths.iter().any(|path| {
            path == "/"
                || path == &prim.path
                || prim
                    .path
                    .strip_prefix(path.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }) {
            continue;
        }
        let Ok(root) = owner_root(
            entity,
            &parents,
            &previews,
            &roots,
            &entities,
            mount.as_deref(),
        ) else {
            continue;
        };
        queue_projection(entity, prim, root, &stages, &canonical, &mut commands);
    }
}

#[cfg(test)]
mod source_lease_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lunco_usd_compose::recipe::StageRecipe;

    fn project(world: &mut World, entity: Entity, root: Entity) {
        world
            .run_system_once(
                move |stages: Res<Assets<UsdStageAsset>>,
                      canonical: NonSend<CanonicalStages>,
                      paths: Query<&UsdPrimPath>,
                      mut commands: Commands| {
                    queue_projection(
                        entity,
                        paths.get(entity).unwrap(),
                        Some(root),
                        &stages,
                        &canonical,
                        &mut commands,
                    );
                },
            )
            .unwrap();
    }

    #[test]
    fn retired_gravity_source_does_not_reject_same_path_stage_replacement() {
        let mut world = World::new();
        world.init_resource::<Assets<UsdStageAsset>>();
        world.init_resource::<PhysicsSceneGravityOwner>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.init_resource::<lunco_core::SceneMountState>();
        world.insert_non_send(CanonicalStages::default());
        let root = world.spawn(UsdSceneRoot).id();
        world
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);
        let mut entities = Vec::new();
        for magnitude in [1, 2] {
            let source = format!(
                "#usda 1.0\n( metersPerUnit = 1\n upAxis = \"Y\" )\ndef PhysicsScene \"Scene\" {{\n float physics:gravityMagnitude = {magnitude}\n vector3f physics:gravityDirection = (0,-1,0)\n}}\n"
            );
            let stage = world.resource_mut::<Assets<UsdStageAsset>>().add(
                UsdStageAsset::from_recipe(StageRecipe::from_source(
                    "inline-gravity.usda",
                    &source,
                ))
                .unwrap(),
            );
            entities.push(
                world
                    .spawn((
                        UsdPhysicsSceneSettings,
                        UsdPrimPath {
                            stage_handle: stage,
                            path: "/Scene".into(),
                        },
                        ChildOf(root),
                    ))
                    .id(),
            );
        }
        project(&mut world, entities[0], root);
        let first = world
            .resource::<PhysicsSceneGravityOwner>()
            .0
            .clone()
            .unwrap();
        assert!(first.is_live(&world));
        let second = GravitySourceLease {
            entity: entities[1],
            stage: world
                .get::<UsdPrimPath>(entities[1])
                .unwrap()
                .stage_handle
                .id(),
            path: first.path.clone(),
            root: Some(root),
        };
        assert!(second.is_live(&world));
        assert_ne!(first.stage, second.stage);
        assert_ne!(first, second);
        let other_root = world.spawn(UsdSceneRoot).id();
        world
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(other_root, false);
        world.entity_mut(entities[0]).insert(ChildOf(other_root));
        assert!(!first.is_live(&world));
        world.entity_mut(entities[0]).insert(ChildOf(root));
        world.entity_mut(root).insert(UsdPreviewOnly);
        assert!(!first.is_live(&world));
        world.entity_mut(root).remove::<UsdPreviewOnly>();
        world
            .resource_mut::<lunco_core::SceneMountState>()
            .begin_replacement();
        assert!(!first.is_live(&world));
        world
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);
        world.get_mut::<UsdPrimPath>(entities[0]).unwrap().path = "/Moved".into();
        assert!(!first.is_live(&world));
        world.get_mut::<UsdPrimPath>(entities[0]).unwrap().path = first.path.clone();
        assert!(first.is_live(&world));
        world.despawn(entities[0]);
        assert!(!first.is_live(&world));
        // The admitted values survive retirement; only their old authority ends.
        assert_eq!(
            world
                .resource::<lunco_environment::PhysicsSceneGravity>()
                .magnitude,
            1.0
        );
        project(&mut world, entities[1], root);
        assert_eq!(
            world.resource::<PhysicsSceneGravityOwner>().0.as_ref(),
            Some(&second)
        );
        assert_eq!(
            world
                .resource::<lunco_environment::PhysicsSceneGravity>()
                .magnitude,
            2.0
        );
        assert_eq!(
            *world.resource::<lunco_environment::Gravity>(),
            lunco_environment::Gravity::flat(2.0, DVec3::NEG_Y)
        );
        assert!(!world.resource::<lunco_core::RuntimeFaults>().active());
    }
}
