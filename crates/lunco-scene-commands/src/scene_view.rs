//! Validated component updates for the mounted scene's presentation.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, Reject};
use lunco_core::{
    ActiveCommandId, CommandOrigin, CommandResults, GlobalEntityId, RuntimeScope, SceneMountState,
    SceneTransitionCoordinator, on_command,
};
use lunco_scene_command_contracts::{SceneViewBatch, SceneViewValue, SetSceneView};
use lunco_scripting_bridge_core::ScriptInstanceRetired;
use lunco_usd_bevy_scene::billboard::{BillboardViewText, UsdBillboard};
use std::collections::{HashMap, HashSet};

/// Register an owner for a named presentation property. Validation reads the
/// current world. Application publishes only that property's runtime state and
/// must be infallible for the value and target that passed validation.
#[derive(Resource, Default)]
pub struct SceneViewProperties(HashMap<&'static str, SceneViewProperty>);

/// Publish a validated presentation value with its optional script owner.
pub type ApplySceneViewProperty = fn(&mut World, Entity, SceneViewValue, Option<GlobalEntityId>);

#[derive(Clone, Copy)]
struct SceneViewProperty {
    validate: fn(&World, Entity, &SceneViewValue) -> Result<(), String>,
    apply: ApplySceneViewProperty,
}

impl SceneViewProperties {
    /// Register a unique name and its validation and application functions.
    pub fn register(
        &mut self,
        name: &'static str,
        validate: fn(&World, Entity, &SceneViewValue) -> Result<(), String>,
        apply: ApplySceneViewProperty,
    ) {
        match self.0.entry(name) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(SceneViewProperty { validate, apply });
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                panic!("duplicate scene view property: {name}");
            }
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SceneViewProperties>()
        .add_observer(retire_script_billboards)
        .add_systems(lunco_core::SceneTeardown, clear_billboard_views);
    app.world_mut()
        .resource_mut::<SceneViewProperties>()
        .register(
            "billboard.text",
            validate_billboard_text,
            apply_billboard_text,
        );
}

#[on_command(SetSceneView)]
pub(crate) fn on_set_scene_view(
    trigger: On<SetSceneView>,
    active: Res<ActiveCommandId>,
    mut commands: Commands,
) {
    let command_id = active.get();
    let origin = active.origin();
    let batches = &trigger.event().batches;
    const MAX_WRITES: usize = 256;
    if batches.len() > MAX_WRITES
        || batches
            .iter()
            .map(|batch| batch.writes.len())
            .sum::<usize>()
            > MAX_WRITES
    {
        commands.queue(move |world: &mut World| {
            finish(
                world,
                command_id,
                Err("SetSceneView accepts at most 256 writes".into()),
            );
        });
        return;
    }
    let batches = batches.clone();
    commands.queue(move |world: &mut World| {
        let result = apply_writes(world, origin, batches).map(|()| Ack::default());
        finish(world, command_id, result);
    });
}

fn finish(world: &mut World, id: Option<u64>, result: Result<Ack, String>) {
    if let Err(detail) = &result {
        warn!("SetSceneView rejected: {detail}");
    }
    if let Some(id) = id {
        world
            .resource_mut::<CommandResults>()
            .record_handler_result(id, result.map_err(Reject::InvalidOp));
    }
}

fn apply_writes(
    world: &mut World,
    origin: Option<CommandOrigin>,
    batches: Vec<SceneViewBatch>,
) -> Result<(), String> {
    let _span = bevy::log::info_span!("apply_scene_view", entities = batches.len()).entered();
    let script = match origin {
        Some(CommandOrigin::Network { .. }) => {
            return Err("SetSceneView changes only the local scene view".into());
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
                return Err("SetSceneView source belongs to a retired scene generation".into());
            }
            actor
        }
        _ => None,
    };
    let mounts = world
        .get_resource::<SceneMountState>()
        .ok_or_else(|| "SetSceneView requires a mounted scene".to_owned())?;
    let registry = world.resource::<SceneViewProperties>();
    let mut targets = HashSet::with_capacity(batches.len());
    let mut last_write = HashMap::new();
    let mut prepared = Vec::new();
    for SceneViewBatch { target, writes } in batches {
        if !targets.insert(target) {
            return Err(format!("duplicate scene view target {target:?}"));
        }
        let root = lunco_usd_bevy_scene::scene_root_in_world(world, target)
            .map_err(|error| format!("cannot resolve scene for {target:?}: {error:?}"))?;
        if !root.is_some_and(|root| mounts.contains_root(root))
            || lunco_usd_bevy_scene::is_preview_only_entity(world, target)
        {
            return Err(format!("{target:?} is outside the mounted scene"));
        }
        for write in writes {
            let property = registry
                .0
                .get_key_value(write.property.as_str())
                .ok_or_else(|| format!("unknown scene view property {:?}", write.property))?;
            let (&name, owner) = property;
            (owner.validate)(world, target, &write.value)?;
            last_write.insert((target, name), prepared.len());
            prepared.push((target, name, owner.apply, write.value));
        }
    }
    for (index, (target, name, apply, value)) in prepared.into_iter().enumerate() {
        if last_write[&(target, name)] == index {
            apply(world, target, value, script);
        }
    }
    Ok(())
}

fn validate_billboard_text(
    world: &World,
    target: Entity,
    value: &SceneViewValue,
) -> Result<(), String> {
    if world.get::<UsdBillboard>(target).is_none() {
        return Err(format!("{target:?} has no enabled billboard"));
    }
    if matches!(value, SceneViewValue::Text { value } if value.len() > 4096) {
        return Err("billboard.text accepts at most 4096 UTF-8 bytes".into());
    }
    Ok(())
}

fn apply_billboard_text(
    world: &mut World,
    target: Entity,
    value: SceneViewValue,
    script: Option<GlobalEntityId>,
) {
    let mut entity = world.entity_mut(target);
    match value {
        SceneViewValue::Text { value } => {
            let next = BillboardViewText {
                template: value,
                script,
            };
            if let Some(mut text) = entity.get_mut::<BillboardViewText>() {
                text.set_if_neq(next);
            } else {
                entity.insert(next);
            }
        }
        SceneViewValue::Clear => {
            if script.is_none()
                || entity
                    .get::<BillboardViewText>()
                    .is_some_and(|text| text.script == script)
            {
                entity.remove::<BillboardViewText>();
            }
        }
    }
}

fn retire_script_billboards(
    retired: On<ScriptInstanceRetired>,
    targets: Query<(Entity, &BillboardViewText)>,
    mut commands: Commands,
) {
    for (entity, text) in &targets {
        if text.script == Some(retired.script) {
            commands.entity(entity).try_remove::<BillboardViewText>();
        }
    }
}

fn clear_billboard_views(mut commands: Commands, targets: Query<Entity, With<BillboardViewText>>) {
    for target in &targets {
        commands.entity(target).try_remove::<BillboardViewText>();
    }
}
