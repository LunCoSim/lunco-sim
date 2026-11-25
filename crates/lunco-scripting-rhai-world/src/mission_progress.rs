//! Per-user mission checkpoints for explicitly resumable, mounted scenarios.

use bevy::prelude::*;
use lunco_scripting_bridge_core as bridge;
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_workspace::{TwinId, WorkspaceResource};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct MissionScope {
    twin: Option<String>,
    scene: String,
    host: String,
    mission: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct MissionRecord {
    scope: MissionScope,
    checkpoints: BTreeMap<String, String>,
}

#[derive(Clone, Default, PartialEq, Resource, Serialize, Deserialize)]
struct MissionProgress {
    missions: Vec<MissionRecord>,
}

impl SettingsSection for MissionProgress {
    const KEY: &'static str = "mission_progress";

    fn validate_section(&self) -> Result<(), String> {
        let mut scopes = BTreeSet::new();
        for record in &self.missions {
            let scope = &record.scope;
            if scope.twin.as_deref().is_some_and(|id| id.trim().is_empty())
                || scope.scene.trim().is_empty()
                || scope.host.trim().is_empty()
                || scope.mission.trim().is_empty()
                || !scopes.insert(scope)
                || record.checkpoints.is_empty()
                || record
                    .checkpoints
                    .iter()
                    .any(|(k, v)| k.trim().is_empty() || v.trim().is_empty())
            {
                return Err(
                    "invalid mission identity, duplicate mission, or empty checkpoint".into(),
                );
            }
        }
        Ok(())
    }
}

/// Install the typed settings section once at runtime composition.
pub fn install(app: &mut App) {
    app.register_settings_section::<MissionProgress>();
}

enum Operation<'a> {
    Read,
    Write(&'a str),
    Clear,
}

fn checkpoint(
    mission: &str,
    key: &str,
    operation: Operation<'_>,
) -> Result<Option<String>, String> {
    if mission.trim().is_empty() || key.trim().is_empty() {
        return Err("mission namespace and checkpoint key must be nonempty".into());
    }
    if matches!(operation, Operation::Write(value) if value.trim().is_empty()) {
        return Err(
            "checkpoint value must be nonempty; use mission_checkpoint_clear to remove it".into(),
        );
    }
    let ctx = bridge::execution_context();
    let route = ctx
        .route
        .ok_or("mission checkpoints require a scenario invocation")?;
    if ctx.phase == lunco_core::RuntimePhase::DependencyPlan {
        return Err("mission checkpoints cannot be used during dependency planning".into());
    }
    bridge::with_world(|world| {
        if !matches!(operation, Operation::Read) {
            bridge::enforce_script_mutation(world, bridge::capability::SETTING_MUTATE, None)?;
        }
        let (scene, host) =
            lunco_scripting_bridge_usd::mounted_host_location(world, bridge::current_self())?;
        let twin = route
            .owner_id
            .map(|id| {
                world
                    .get_resource::<WorkspaceResource>()
                    .and_then(|workspace| workspace.twin(TwinId::new(id)))
                    .map(lunco_assets_core::logical_twin_source_identity)
                    .ok_or("mission's owning Twin is no longer mounted")
            })
            .transpose()?;
        let scope = MissionScope {
            twin,
            scene,
            host,
            mission: mission.to_owned(),
        };
        let mut progress = world
            .get_resource_mut::<MissionProgress>()
            .ok_or("mission progress is unavailable")?;
        let index = progress
            .missions
            .iter()
            .position(|record| record.scope == scope);
        match operation {
            Operation::Read => {
                Ok(index.and_then(|i| progress.missions[i].checkpoints.get(key).cloned()))
            }
            Operation::Write(value) => {
                if let Some(i) = index {
                    if progress.missions[i]
                        .checkpoints
                        .get(key)
                        .is_none_or(|old| old != value)
                    {
                        progress.missions[i]
                            .checkpoints
                            .insert(key.into(), value.into());
                    }
                } else {
                    progress.missions.push(MissionRecord {
                        scope,
                        checkpoints: BTreeMap::from([(key.into(), value.into())]),
                    });
                }
                Ok(None)
            }
            Operation::Clear => {
                if let Some(i) = index
                    && progress.missions[i].checkpoints.contains_key(key)
                {
                    progress.missions[i].checkpoints.remove(key);
                    if progress.missions[i].checkpoints.is_empty() {
                        progress.missions.remove(i);
                    }
                }
                Ok(None)
            }
        }
    })
    .ok_or("mission checkpoints require a live World")?
}

pub(crate) fn register(engine: &mut Engine) {
    engine.register_fn(
        "mission_checkpoint_read",
        |mission: ImmutableString, key: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            Ok(checkpoint(&mission, &key, Operation::Read)?
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT))
        },
    );
    engine.register_fn(
        "mission_checkpoint_write",
        |mission: ImmutableString,
         key: ImmutableString,
         value: ImmutableString|
         -> Result<(), Box<EvalAltResult>> {
            checkpoint(&mission, &key, Operation::Write(&value))?;
            Ok(())
        },
    );
    engine.register_fn(
        "mission_checkpoint_clear",
        |mission: ImmutableString, key: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            checkpoint(&mission, &key, Operation::Clear)?;
            Ok(())
        },
    );
}
