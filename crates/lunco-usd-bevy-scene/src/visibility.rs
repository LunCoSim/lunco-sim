//! Runtime visibility opinions use the authored USD namespace, not the physical
//! ECS parent tree (which can be reparented to implement resetXformStack).
//! A single root override is enough to reveal a group; descendants retain their
//! own authored pruning opinions and native purpose visibility.

use crate::{
    NativeViewVisibility, RuntimeVisibilityOverride, UsdPrimPath, UsdSampledVisibility,
    UsdSceneProjectionFailed, UsdVisualVisibility, prim_index::UsdPrimIndex, scene_root_in_world,
};
use bevy::prelude::*;
use lunco_usd_bevy_stage::{
    UsdInstanceMember, UsdInstanceRoot, UsdRead, UsdStageAsset, canonical::CanonicalStages,
};
use openusd::sdf::Path;
use std::collections::{BTreeSet, HashMap};

/// Validate the presentation origin before a local view mutation is admitted.
pub fn presentation_actor(
    world: &World,
    origin: Option<lunco_core::CommandOrigin>,
) -> Result<Option<lunco_core::GlobalEntityId>, String> {
    use lunco_core::{CommandOrigin, RuntimeScope, SceneTransitionCoordinator};
    match origin {
        Some(CommandOrigin::Network { .. }) => {
            Err("presentation controls change only the local scene view".into())
        }
        Some(CommandOrigin::Rhai { context, actor, .. }) => {
            if let Some(route) = context
                .route
                .filter(|route| route.scope == RuntimeScope::Twin)
                && route.generation != 0
                && world
                    .get_resource::<SceneTransitionCoordinator>()
                    .and_then(SceneTransitionCoordinator::completed_generation)
                    != Some(route.generation)
            {
                return Err("presentation source belongs to a retired scene generation".into());
            }
            Ok(actor)
        }
        _ => Ok(None),
    }
}

/// Validate the exact live USD target, not an authoring preview or a retired
/// scene. Its visibility must have an admitted independent channel.
pub fn validate_runtime_visibility(world: &World, target: Entity) -> Result<(), String> {
    let root = scene_root_in_world(world, target)
        .map_err(|error| format!("cannot resolve scene for {target:?}: {error:?}"))?;
    if !root.is_some_and(|root| {
        world
            .get_resource::<lunco_core::SceneMountState>()
            .is_some_and(|mounts| mounts.contains_root(root))
    }) || crate::is_preview_only_entity(world, target)
    {
        return Err(format!("{target:?} is outside the mounted scene"));
    }
    if world.get::<NativeViewVisibility>(target).is_some() {
        return world
            .get::<Visibility>(target)
            .map(|_| ())
            .ok_or_else(|| format!("{target:?} has no native visibility component"));
    }
    if world
        .get::<UsdVisualVisibility>(target)
        .is_none_or(|state| state.animated || state.failure.is_some())
        || world.get::<Visibility>(target).is_none()
        || world.get::<crate::UsdSceneProjected>(target).is_none()
        || world.get::<UsdSceneProjectionFailed>(target).is_some()
    {
        return Err(format!(
            "{target:?} has no independently controllable projected visibility"
        ));
    }
    let path = world
        .get::<UsdPrimPath>(target)
        .ok_or("visibility target has no USD identity")?;
    let stages = world
        .get_non_send::<CanonicalStages>()
        .ok_or("USD stage registry is unavailable")?;
    let asset = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|assets| assets.get(&path.stage_handle))
        .ok_or("visibility target's stage asset is unavailable")?;
    let (reader, _) = stages.reader_for_composed_query(
        path.stage_handle.id(),
        asset,
        lunco_usd_bevy_stage::canonical::ComposedReadRequirement::Native,
    );
    let prim = Path::new(&path.path).map_err(|e| e.to_string())?;
    if !reader.has_prim(&prim) {
        return Err(format!(
            "visibility target {} is absent from the current composed stage",
            path.path
        ));
    }
    Ok(())
}

/// Publish a validated root opinion. `None` withdraws the actor's own opinion.
/// The caller reconciles all changed roots once the complete batch is applied.
pub fn write_runtime_visibility(
    world: &mut World,
    target: Entity,
    value: Option<bool>,
    script: Option<lunco_core::GlobalEntityId>,
) -> bool {
    let mut entity = world.entity_mut(target);
    match value {
        Some(visible) => {
            let next = RuntimeVisibilityOverride { visible, script };
            if let Some(mut existing) = entity.get_mut::<RuntimeVisibilityOverride>() {
                existing.set_if_neq(next)
            } else {
                entity.insert(next);
                true
            }
        }
        None => {
            if entity
                .get::<RuntimeVisibilityOverride>()
                .is_some_and(|value| script.is_none() || value.script == script)
            {
                entity.remove::<RuntimeVisibilityOverride>();
                true
            } else {
                false
            }
        }
    }
}

/// Identify the precise projected namespace instance, including spawned copies.
fn scope(world: &World, entity: Entity) -> Option<(Entity, Option<Entity>)> {
    let root = scene_root_in_world(world, entity).ok().flatten()?;
    let instance = world
        .get::<UsdInstanceMember>(entity)
        .map(|member| member.root)
        .or_else(|| world.get::<UsdInstanceRoot>(entity).map(|_| entity));
    Some((root, instance))
}

/// Apply committed runtime visibility opinions only to affected projected
/// namespaces. Multiple roots in one command are reconciled after all opinions
/// have been written. No USD transactions, mesh jobs, or physics changes occur.
///
/// Authored visibility errors stay owned by the USD projection. We never reveal
/// a prim that another projection owner has withheld.
pub fn reconcile_runtime_visibility(world: &mut World, roots: &[Entity]) {
    for &root in roots {
        if let Some(base) = world.get::<NativeViewVisibility>(root) {
            let visible = world
                .get::<RuntimeVisibilityOverride>(root)
                .map_or(base.0, |value| value.visible);
            if let Some(mut visibility) = world.get_mut::<Visibility>(root) {
                visibility.set_if_neq(if visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
            }
        }
    }
    let facts = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let index = world.resource::<UsdPrimIndex>();
        let mut affected = BTreeSet::new();
        let mut scopes = HashMap::new();
        let mut cached_scope =
            |entity| *scopes.entry(entity).or_insert_with(|| scope(world, entity));
        let mut branches = HashMap::<_, BTreeSet<String>>::new();
        for &root in roots {
            let Some(path) = world.get::<UsdPrimPath>(root) else {
                continue;
            };
            let Some(root_scope) = cached_scope(root) else {
                continue;
            };
            branches
                .entry((path.stage_handle.id(), root_scope))
                .or_default()
                .insert(path.path.clone());
        }
        for ((stage, root_scope), paths) in branches {
            let mut covered: Option<String> = None;
            for path in paths {
                if covered.as_ref().is_some_and(|prefix| {
                    path.strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                }) {
                    continue;
                }
                for entity in index.in_subtree(stage, &path) {
                    if cached_scope(entity) == Some(root_scope) {
                        affected.insert(entity);
                    }
                }
                covered = Some(path);
            }
        }
        let mut updates = Vec::new();
        let mut ancestor_overrides = HashMap::new();
        for entity in affected {
            let Some(state) = world.get::<UsdVisualVisibility>(entity) else {
                continue;
            };
            if state.failure.is_some() || world.get::<UsdSceneProjectionFailed>(entity).is_some() {
                continue;
            }
            let Some(prim) = world.get::<UsdPrimPath>(entity) else {
                continue;
            };
            let Some(target_scope) = cached_scope(entity) else {
                continue;
            };
            let Ok(path) = Path::new(&prim.path) else {
                continue;
            };
            let mut overrides = HashMap::new();
            for ancestor in path.ancestors_below_root() {
                let value = *ancestor_overrides
                    .entry((prim.stage_handle.id(), target_scope, ancestor.clone()))
                    .or_insert_with(|| {
                        index
                            .in_stage(prim.stage_handle.id(), ancestor.as_str())
                            .iter()
                            .find_map(|&candidate| {
                                if cached_scope(candidate) != Some(target_scope) {
                                    return None;
                                }
                                if world
                                    .get::<UsdVisualVisibility>(candidate)
                                    .is_some_and(|state| state.animated)
                                {
                                    world
                                        .get::<UsdSampledVisibility>(candidate)
                                        .map(|sample| sample.0)
                                } else {
                                    world
                                        .get::<RuntimeVisibilityOverride>(candidate)
                                        .map(|value| value.visible)
                                }
                            })
                    });
                if let Some(value) = value {
                    overrides.insert(ancestor.to_string(), value);
                }
            }
            // False opinions prune immediately. True opinions cannot change
            // a surface already admitted by authored visibility. Only a reveal
            // of an authored-hidden prim needs the native schema walk.
            let value = if overrides.values().any(|value| !*value) {
                Ok(Visibility::Hidden)
            } else if overrides.is_empty() || state.authored != Visibility::Hidden {
                Ok(state.authored)
            } else {
                world
                    .get_resource::<Assets<UsdStageAsset>>()
                    .and_then(|assets| assets.get(&prim.stage_handle))
                    .ok_or_else(|| format!("{}: stage asset is unavailable", prim.path))
                    .and_then(|asset| {
                        stages
                            .reader_for_composed_query(
                                prim.stage_handle.id(),
                                asset,
                                lunco_usd_bevy_stage::canonical::ComposedReadRequirement::Native,
                            )
                            .0
                            .invisible_with_overrides(&path, &overrides)
                    })
                    .map(|hidden| {
                        if hidden {
                            Visibility::Hidden
                        } else {
                            Visibility::Inherited
                        }
                    })
            };
            updates.push((entity, value));
        }
        updates
    };
    for (entity, value) in facts {
        match value {
            Ok(value) => {
                if let Some(mut visibility) = world.get_mut::<Visibility>(entity) {
                    visibility.set_if_neq(value);
                }
            }
            Err(message) => {
                if let Some(mut visibility) = world.get_mut::<Visibility>(entity) {
                    visibility.set_if_neq(Visibility::Hidden);
                }
                error!("[usd-visibility] {message}");
                world.trigger(lunco_core::RuntimeError {
                    name: "usd-runtime-visibility-failed".to_owned(),
                    message,
                });
            }
        }
    }
}

/// Visibility reconciliation consumes animation samples before render propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsdVisibilityReconcileSet;

/// One native visual projection was added or replaced while a runtime
/// namespace opinion was active. Coalesce these before visibility propagation.
#[derive(Message, Clone, Copy)]
pub struct VisibilityProjectionChanged(pub Entity);

pub(crate) fn queue_visibility_projection(
    inserted: On<Insert<UsdVisualVisibility>>,
    active: Query<(), Or<(With<RuntimeVisibilityOverride>, With<UsdSampledVisibility>)>>,
    mut messages: MessageWriter<VisibilityProjectionChanged>,
) {
    if !active.is_empty() {
        messages.write(VisibilityProjectionChanged(inserted.entity));
    }
}

/// Newly admitted descendants inherit any live ancestor opinion; initial
/// projection pays no native walk when none is active.
pub(crate) fn reconcile_new_visibility_projections(
    mut messages: MessageReader<VisibilityProjectionChanged>,
    mut commands: Commands,
) {
    let roots: Vec<_> = messages.read().map(|message| message.0).collect();
    if !roots.is_empty() {
        commands.queue(move |world: &mut World| reconcile_runtime_visibility(world, &roots));
    }
}
