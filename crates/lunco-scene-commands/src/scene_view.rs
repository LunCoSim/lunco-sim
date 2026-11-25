//! Validated component updates for the mounted scene's presentation.

use bevy::prelude::*;
use lunco_command_contracts::Ack;
use lunco_core::{ActiveCommandId, CommandOrigin, GlobalEntityId, SceneMountState, on_command};
use lunco_render::{PbrLook, PbrViewColor, ViewColorChannel};
use lunco_scene_command_contracts::{SceneViewBatch, SceneViewValue, SetSceneView};
use lunco_scripting_bridge_core::ScriptInstanceRetired;
use lunco_usd_bevy_scene::billboard::{BillboardViewText, UsdBillboard};
use lunco_usd_bevy_scene::{
    RuntimeVisibilityOverride,
    visibility::{
        presentation_actor, reconcile_runtime_visibility, validate_runtime_visibility,
        write_runtime_visibility,
    },
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Register an owner for a named presentation property. Validation reads the
/// current world. Application publishes only that property's runtime state and
/// must be infallible for the value and target that passed validation.
#[derive(Resource, Default)]
pub struct SceneViewProperties(HashMap<&'static str, SceneViewProperty>);

/// Publish a validated presentation value with its optional script owner.
/// Return whether the value changed and needs derived presentation reconciliation.
pub type ApplySceneViewProperty =
    fn(&mut World, Entity, SceneViewValue, Option<GlobalEntityId>) -> bool;

#[derive(Clone, Copy)]
struct SceneViewProperty {
    validate: fn(&World, Entity, &SceneViewValue) -> Result<(), String>,
    apply: ApplySceneViewProperty,
    reconcile: Option<fn(&mut World, &[Entity])>,
}

impl SceneViewProperties {
    /// Register a unique name and its validation and application functions.
    pub fn register(
        &mut self,
        name: &'static str,
        validate: fn(&World, Entity, &SceneViewValue) -> Result<(), String>,
        apply: ApplySceneViewProperty,
        reconcile: Option<fn(&mut World, &[Entity])>,
    ) {
        match self.0.entry(name) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(SceneViewProperty {
                    validate,
                    apply,
                    reconcile,
                });
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                panic!("duplicate scene view property: {name}");
            }
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SceneViewProperties>()
        .register_type::<PbrLook>()
        .register_type::<PbrViewColor>()
        .add_observer(retire_script_colors)
        .add_observer(transfer_split_color)
        .add_observer(retire_script_billboards)
        .add_observer(retire_script_visibility)
        .add_systems(
            lunco_core::SceneTeardown,
            (
                clear_billboard_views,
                clear_visibility_views,
                clear_color_views,
            ),
        );
    app.world_mut()
        .resource_mut::<SceneViewProperties>()
        .register(
            "billboard.text",
            validate_billboard_text,
            apply_billboard_text,
            None,
        );
    app.world_mut()
        .resource_mut::<SceneViewProperties>()
        .register(
            "visibility",
            validate_visibility,
            apply_visibility,
            Some(reconcile_runtime_visibility),
        );
    app.world_mut()
        .resource_mut::<SceneViewProperties>()
        .register(
            "material.base_color",
            validate_color,
            |world, target, value, script| {
                apply_color(world, target, value, script, |color| &mut color.base_color)
            },
            None,
        );
    app.world_mut()
        .resource_mut::<SceneViewProperties>()
        .register(
            "material.emissive",
            validate_color,
            |world, target, value, script| {
                apply_color(world, target, value, script, |color| &mut color.emissive)
            },
            None,
        );
}

#[on_command(SetSceneView)]
pub(crate) fn on_set_scene_view(
    trigger: On<SetSceneView>,
    active: Res<ActiveCommandId>,
    reply: lunco_api::executor::CommandReply,
    mut commands: Commands,
) -> lunco_core::CommandOutcome {
    let origin = active.origin();
    let batches = trigger.event().batches.clone();
    reply.queue(&mut commands, move |world| {
        if batches.len() > 256
            || batches
                .iter()
                .map(|batch| batch.writes.len())
                .sum::<usize>()
                > 256
        {
            return Err("SetSceneView accepts at most 256 writes".into());
        }
        apply_writes(world, origin, batches)?;
        Ok(Ack::default())
    })
}

pub(crate) fn apply_writes(
    world: &mut World,
    origin: Option<CommandOrigin>,
    batches: Vec<SceneViewBatch>,
) -> Result<(), String> {
    let _span = bevy::log::info_span!("apply_scene_view", entities = batches.len()).entered();
    let script = presentation_actor(world, origin)?;
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
            prepared.push((target, name, owner.apply, owner.reconcile, write.value));
        }
    }
    let mut reconcile_by_owner = BTreeMap::new();
    for (index, (target, name, apply, reconcile, value)) in prepared.into_iter().enumerate() {
        if last_write[&(target, name)] == index {
            if apply(world, target, value, script)
                && let Some(reconcile) = reconcile
            {
                reconcile_by_owner
                    .entry(name)
                    .or_insert_with(|| (reconcile, Vec::new()))
                    .1
                    .push(target);
            }
        }
    }
    for (_, (reconcile, targets)) in reconcile_by_owner {
        reconcile(world, &targets);
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
    if !matches!(value, SceneViewValue::Text { .. } | SceneViewValue::Clear) {
        return Err("billboard.text requires Text or Clear".into());
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
) -> bool {
    let mut entity = world.entity_mut(target);
    match value {
        SceneViewValue::Text { value } => {
            let next = BillboardViewText {
                template: value,
                script,
            };
            if let Some(mut text) = entity.get_mut::<BillboardViewText>() {
                text.set_if_neq(next)
            } else {
                entity.insert(next);
                true
            }
        }
        SceneViewValue::Bool { .. } | SceneViewValue::LinearRgb { .. } => {
            unreachable!("validated billboard text")
        }
        SceneViewValue::Clear => {
            if script.is_none()
                || entity
                    .get::<BillboardViewText>()
                    .is_some_and(|text| text.script == script)
            {
                let changed = entity.contains::<BillboardViewText>();
                entity.remove::<BillboardViewText>();
                changed
            } else {
                false
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

/// Validate a local presentation visibility write without admitting new geometry.
fn validate_visibility(
    world: &World,
    target: Entity,
    value: &SceneViewValue,
) -> Result<(), String> {
    if !matches!(value, SceneViewValue::Bool { .. } | SceneViewValue::Clear) {
        return Err("visibility requires Bool or Clear".into());
    }
    validate_runtime_visibility(world, target)
}

/// Apply the runtime override; clearing it restores the current composed value.
fn apply_visibility(
    world: &mut World,
    target: Entity,
    value: SceneViewValue,
    script: Option<GlobalEntityId>,
) -> bool {
    let value = match value {
        SceneViewValue::Bool { value } => Some(value),
        SceneViewValue::Clear => None,
        SceneViewValue::Text { .. } | SceneViewValue::LinearRgb { .. } => {
            unreachable!("validated visibility")
        }
    };
    write_runtime_visibility(world, target, value, script)
}

/// Restore authored visibility when the publishing scenario retires.
fn retire_script_visibility(
    retired: On<ScriptInstanceRetired>,
    targets: Query<(Entity, &RuntimeVisibilityOverride)>,
    mut commands: Commands,
) {
    let owner = retired.script;
    let roots = targets
        .iter()
        .filter_map(|(entity, value)| (value.script == Some(owner)).then_some(entity))
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        for &entity in &roots {
            if world
                .get::<RuntimeVisibilityOverride>(entity)
                .is_some_and(|value| value.script == Some(owner))
            {
                world
                    .entity_mut(entity)
                    .remove::<RuntimeVisibilityOverride>();
            }
        }
        reconcile_runtime_visibility(world, &roots);
    });
}

/// Clear scene-local presentation overrides at teardown.
fn clear_visibility_views(world: &mut World) {
    let roots = world
        .query_filtered::<Entity, With<RuntimeVisibilityOverride>>()
        .iter(world)
        .collect::<Vec<_>>();
    for &entity in &roots {
        world
            .entity_mut(entity)
            .remove::<RuntimeVisibilityOverride>();
    }
    reconcile_runtime_visibility(world, &roots);
}

fn color_target(world: &World, target: Entity) -> Entity {
    world
        .get::<lunco_usd_bevy_scene::UsdVisualMeshTarget>(target)
        .map_or(target, |mesh| mesh.0)
}

fn validate_color(world: &World, target: Entity, value: &SceneViewValue) -> Result<(), String> {
    let target = color_target(world, target);
    if !world.entities().contains(target) {
        return Err("material draw entity is no longer live".into());
    }
    match value {
        SceneViewValue::Clear => Ok(()),
        SceneViewValue::LinearRgb { value } if value.iter().all(|v| v.is_finite() && *v >= 0.0) => {
            world
                .get::<PbrLook>(target)
                .map(|_| ())
                .ok_or_else(|| format!("{target:?} has no PBR appearance"))
        }
        _ => Err("PBR colour requires a finite non-negative LinearRgb or Clear".into()),
    }
}

fn apply_color(
    world: &mut World,
    target: Entity,
    value: SceneViewValue,
    script: Option<GlobalEntityId>,
    channel: fn(&mut PbrViewColor) -> &mut Option<ViewColorChannel>,
) -> bool {
    let target = color_target(world, target);
    let mut entity = world.entity_mut(target);
    let old = entity.get::<PbrViewColor>().copied().unwrap_or_default();
    let mut new = old;
    let slot = channel(&mut new);
    match value {
        SceneViewValue::LinearRgb { value: rgb } => *slot = Some(ViewColorChannel { rgb, script }),
        SceneViewValue::Clear if script.is_none() || slot.is_some_and(|c| c.script == script) => {
            *slot = None
        }
        SceneViewValue::Clear => return false,
        _ => unreachable!("validated PBR colour"),
    }
    if new == old {
        return false;
    }
    if new == PbrViewColor::default() {
        entity.remove::<PbrViewColor>();
    } else {
        entity.insert(new);
    }
    true
}

fn transfer_split_color(
    split: On<Insert<lunco_usd_bevy_scene::UsdVisualMeshTarget>>,
    targets: Query<(&lunco_usd_bevy_scene::UsdVisualMeshTarget, &PbrViewColor)>,
    mut commands: Commands,
) {
    if let Ok((mesh, color)) = targets.get(split.entity) {
        commands.entity(mesh.0).try_insert(*color);
        commands.entity(split.entity).try_remove::<PbrViewColor>();
    }
}

fn retire_script_colors(
    retired: On<ScriptInstanceRetired>,
    targets: Query<(Entity, &PbrViewColor)>,
    mut commands: Commands,
) {
    let owner = Some(retired.script);
    for (target, color) in &targets {
        if [color.base_color, color.emissive]
            .into_iter()
            .flatten()
            .any(|channel| channel.script == owner)
        {
            commands.queue(move |world: &mut World| {
                let Ok(mut entity) = world.get_entity_mut(target) else {
                    return;
                };
                let Some(current) = entity.get::<PbrViewColor>() else {
                    return;
                };
                let next = PbrViewColor {
                    base_color: current.base_color.filter(|channel| channel.script != owner),
                    emissive: current.emissive.filter(|channel| channel.script != owner),
                };
                if next == *current {
                    return;
                }
                if next == PbrViewColor::default() {
                    entity.remove::<PbrViewColor>();
                } else {
                    entity.insert(next);
                }
            });
        }
    }
}

fn clear_color_views(targets: Query<Entity, With<PbrViewColor>>, mut commands: Commands) {
    for target in &targets {
        commands.entity(target).try_remove::<PbrViewColor>();
    }
}
