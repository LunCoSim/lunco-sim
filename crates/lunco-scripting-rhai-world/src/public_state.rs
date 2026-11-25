//! Typed snapshots explicitly published by a live scenario instance.

use bevy::prelude::*;
use lunco_api::registry::ApiEntityRegistry;
use lunco_hooks::HookValue;
use lunco_scripting::{ScriptedModel, TwinOwnedScript};
use lunco_scripting_bridge_core as bridge;
use std::collections::BTreeMap;

/// Stored on the actor so a host despawn releases every published value.
#[derive(Component, Default)]
struct ScenarioPublicState(BTreeMap<String, HookValue>);

pub fn install(app: &mut App) {
    app.add_observer(retire_instance)
        .add_observer(retire_attachment)
        .add_observer(retire_twin)
        .add_systems(lunco_core::SceneTeardown, retire_scene);
}

fn retire_instance(
    retired: On<bridge::ScriptInstanceRetired>,
    registry: Option<Res<ApiEntityRegistry>>,
    mut states: Query<&mut ScenarioPublicState>,
) {
    if let Some(host) = registry.and_then(|registry| registry.resolve(&retired.script))
        && let Ok(mut state) = states.get_mut(host)
    {
        state.0.clear();
    }
}

fn retire_attachment(
    removed: On<Remove<ScriptedModel>>,
    mut states: Query<&mut ScenarioPublicState>,
) {
    if let Ok(mut state) = states.get_mut(removed.entity) {
        state.0.clear();
    }
}

fn retire_twin(
    closed: On<lunco_workspace::TwinClosed>,
    mut states: Query<(&TwinOwnedScript, &mut ScenarioPublicState)>,
) {
    for (owner, mut state) in &mut states {
        if owner.twin == closed.twin {
            state.0.clear();
        }
    }
}

fn retire_scene(mut commands: Commands, states: Query<Entity, With<ScenarioPublicState>>) {
    for host in &states {
        commands.entity(host).try_remove::<ScenarioPublicState>();
    }
}

pub(crate) fn publish(key: &str, value: HookValue) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("scenario state requires a nonempty key".into());
    }
    let gid = bridge::executing_scenario()
        .ok_or("publish_scenario_state requires a running scenario instance")?;
    if bridge::execution_context().phase == lunco_core::RuntimePhase::Stop {
        return Err("a retiring scenario cannot publish state".into());
    }
    bridge::with_world(|world| {
        bridge::enforce_script_mutation(world, bridge::capability::FIELD_MUTATE, Some(gid))?;
        let host = bridge::resolve_entity(world, gid).ok_or("scenario host is no longer live")?;
        if world.get::<ScriptedModel>(host).is_none() {
            return Err("scenario host no longer has an attached program".into());
        }
        if let Some(mut state) = world.get_mut::<ScenarioPublicState>(host) {
            if value == HookValue::Unit {
                state.0.remove(key);
            } else if state.0.get(key) != Some(&value) {
                state.0.insert(key.into(), value);
            }
        } else if value != HookValue::Unit {
            world
                .entity_mut(host)
                .insert(ScenarioPublicState(BTreeMap::from([(key.into(), value)])));
        }
        Ok(())
    })
    .ok_or("scenario state requires a World")?
}

pub(crate) fn read(gid: u64, key: &str) -> Result<HookValue, String> {
    if key.trim().is_empty() {
        return Err("scenario state requires a nonempty key".into());
    }
    bridge::with_world(|world| {
        bridge::validate_simulation_entity_access(
            world,
            Some(gid),
            "scenario_public_state",
            bridge::ScriptEntityAccess::Read,
        )?;
        let Some(host) = bridge::resolve_entity(world, gid) else {
            return Ok(HookValue::Unit);
        };
        let target_twin = bridge::execution_context()
            .target_twin()
            .map_err(|err| err.to_string())?;
        if let Some(owner) = world.get::<TwinOwnedScript>(host)
            && let Some(twin) = target_twin
            && twin != owner.twin.raw()
        {
            return Err("scenario state belongs to a different Twin".into());
        }
        Ok(world
            .get::<ScenarioPublicState>(host)
            .and_then(|state| state.0.get(key))
            .cloned()
            .unwrap_or(HookValue::Unit))
    })
    .ok_or("scenario state requires a World")?
}
