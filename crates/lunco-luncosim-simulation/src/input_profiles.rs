//! Input documents are admitted through the shared asynchronous asset scope.
use bevy::prelude::*;
use lunco_assets_runtime::{JsonAssetContent, JsonAssetScopeChanged};
use lunco_core::AppSceneExt;
use lunco_input_core::{InputBindingsSettings, SceneInputBindings};
use lunco_workspace::{TwinClosed, TwinId, WorkspaceResource};
use std::collections::BTreeMap;

const KIND: &str = "lunco.input-bindings.v1";

/// Sources belong to open Twins. Scene teardown republishes the selected profile;
/// TwinClosed removes its source before the identity can be reused.
#[derive(Resource, Default)]
struct InputProfiles(BTreeMap<TwinId, Result<Option<InputBindingsSettings>, String>>);
impl lunco_core::SceneResource for InputProfiles {
    fn clear_scene(&mut self) {}
}

pub(super) struct InputProfilesPlugin;
impl Plugin for InputProfilesPlugin {
    fn build(&self, app: &mut App) {
        app.init_scene_resource::<InputProfiles>()
            .add_observer(admit_input_profile)
            .add_observer(retire_input_profile)
            .add_systems(
                PreUpdate,
                select_input_profile.before(lunco_input_core::InputBindingsSet),
            );
    }
}

fn admit_input_profile(
    event: On<JsonAssetScopeChanged>,
    workspace: Option<Res<WorkspaceResource>>,
    mut profiles: ResMut<InputProfiles>,
    mut settings: ResMut<InputBindingsSettings>,
    mut commands: Commands,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    let scope = event.event();
    if let Some(id) = scope.twin_id {
        if !workspace.as_ref().is_some_and(|w| w.twin(id).is_some()) {
            return;
        }
        profiles.0.remove(&id);
    }
    let result = (|| -> Result<Option<InputBindingsSettings>, String> {
        let mut selected = None;
        for asset in &scope.assets {
            let JsonAssetContent::Source(Ok(text)) = &asset.content else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            if value.get("kind").and_then(|v| v.as_str()) != Some(KIND) {
                continue;
            }
            if selected.is_some() {
                return Err(format!(
                    "{} contains multiple {KIND} documents",
                    scope.asset_root_uri
                ));
            }
            let mut profile = InputBindingsSettings::from_json(text)
                .map_err(|e| format!("{}: {e}", asset.asset_uri))?;
            if value.get("look_button").is_none() {
                profile.look_button = None;
            }
            selected = Some(profile);
        }
        if scope.twin_id.is_none() && selected.is_none() {
            return Err(format!(
                "{} contains no {KIND} document",
                scope.asset_root_uri
            ));
        }
        Ok(selected)
    })();
    if let Some(id) = scope.twin_id {
        profiles.0.insert(id, result);
        return;
    }
    match result {
        Ok(Some(profile)) => {
            // User preferences override application defaults. Twin profiles
            // are composed separately and never enter the persisted section.
            let resolved = profile.overridden_by(&settings);
            settings.set_if_neq(resolved);
        }
        Ok(None) => {}
        Err(error) => {
            faults.raise("input-profile-invalid", None, &scope.asset_root_uri, &error);
            lunco_core::trigger_runtime_error(&mut commands, "input-profile-invalid", error);
        }
    }
}

fn retire_input_profile(
    event: On<TwinClosed>,
    mut profiles: ResMut<InputProfiles>,
    mut scene: ResMut<SceneInputBindings>,
) {
    profiles.0.remove(&event.twin);
    if event.was_active {
        scene.0 = None;
    }
}

fn select_input_profile(
    profiles: Res<InputProfiles>,
    workspace: Option<Res<WorkspaceResource>>,
    mut scene: ResMut<SceneInputBindings>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut commands: Commands,
) {
    if !profiles.is_changed()
        && !scene.is_changed()
        && !workspace.as_ref().is_some_and(|w| w.is_changed())
    {
        return;
    }
    let selected = workspace
        .as_ref()
        .and_then(|w| w.active_twin)
        .and_then(|id| profiles.0.get(&id));
    let profile = match selected {
        Some(Ok(profile)) => profile.clone(),
        Some(Err(error)) => {
            if faults.raise(
                "input-profile-invalid",
                None,
                "active Twin input profile",
                error,
            ) {
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    "input-profile-invalid",
                    error.clone(),
                );
            }
            None
        }
        None => None,
    };
    scene.set_if_neq(SceneInputBindings(profile));
}
