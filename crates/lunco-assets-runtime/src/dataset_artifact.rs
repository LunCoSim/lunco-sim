//! Asynchronous reads of policy-selected UTF-8 dataset artifacts.
//!
//! Rhai selects declared dataset ids. This plugin turns each selection into a
//! canonical asset URI and uses Bevy's shared text asset source for loading.

use bevy::asset::{AssetEvent, AssetLoadFailedEvent};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::TextAsset;

/// Ask the shared asset runtime to load a declared text dataset by registry id.
#[derive(Event, Clone, Debug)]
pub struct ReadDatasetTextArtifact {
    /// Dataset registry identity selected by authored policy.
    pub id: String,
}

/// Text for one dataset selected by authored asset policy.
#[derive(Event, Clone, Debug)]
pub struct DatasetTextArtifactReady {
    /// Registry identity selected by policy.
    pub id: String,
    /// Delivered UTF-8 text.
    pub text: String,
}

/// Installs generic reads of Rhai-selected text dataset artifacts.
pub struct DatasetArtifactPlugin;

impl Plugin for DatasetArtifactPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::TextAssetPlugin>() {
            app.add_plugins(crate::TextAssetPlugin);
        }
        app.init_resource::<DatasetArtifactReads>()
            .add_observer(read_requested_dataset_artifact)
            .add_observer(retry_installed_dataset_artifact)
            .add_observer(retire_scene_dataset_reads)
            .add_systems(Update, publish_dataset_artifact_reads);
    }
}

#[derive(Resource, Default)]
struct DatasetArtifactReads {
    scene_generation: u64,
    requested: HashSet<String>,
    delivered: HashSet<String>,
    failed: HashSet<String>,
    handles: HashMap<AssetId<TextAsset>, Vec<PendingDatasetArtifactRead>>,
}

struct PendingDatasetArtifactRead {
    id: String,
    scene_generation: u64,
    asset_uri: String,
    _handle: Handle<TextAsset>,
}

fn retire_scene_dataset_reads(
    _trigger: On<lunco_core::SceneTransitionStarted>,
    mut reads: ResMut<DatasetArtifactReads>,
) {
    reads.scene_generation = reads.scene_generation.wrapping_add(1);
    reads.requested.clear();
    reads.delivered.clear();
    reads.failed.clear();
    reads.handles.clear();
}

fn read_requested_dataset_artifact(
    trigger: On<ReadDatasetTextArtifact>,
    registry: Option<Res<lunco_assets_datasets::DatasetRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<TextAsset>>>,
    mut reads: ResMut<DatasetArtifactReads>,
    mut commands: Commands,
) {
    request_dataset_artifact(
        &trigger.event().id,
        registry.as_deref(),
        asset_server.as_deref(),
        assets.as_deref(),
        &mut reads,
        &mut commands,
        false,
    );
}

fn retry_installed_dataset_artifact(
    trigger: On<lunco_assets_datasets::DatasetInstalled>,
    registry: Option<Res<lunco_assets_datasets::DatasetRegistry>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<TextAsset>>>,
    mut reads: ResMut<DatasetArtifactReads>,
    mut commands: Commands,
) {
    let id = &trigger.event().id;
    if !reads.requested.contains(id) {
        return;
    }
    request_dataset_artifact(
        id,
        registry.as_deref(),
        asset_server.as_deref(),
        assets.as_deref(),
        &mut reads,
        &mut commands,
        true,
    );
}

fn request_dataset_artifact(
    id: &str,
    registry: Option<&lunco_assets_datasets::DatasetRegistry>,
    asset_server: Option<&AssetServer>,
    assets: Option<&Assets<TextAsset>>,
    reads: &mut DatasetArtifactReads,
    commands: &mut Commands,
    retry: bool,
) {
    let Some(registry) = registry else {
        report_dataset_request_failure(
            id,
            "the dataset registry is unavailable".to_owned(),
            reads,
            commands,
        );
        return;
    };
    let Some(entry) = registry.entry(id) else {
        report_dataset_request_failure(
            id,
            format!("no dataset is declared with id '{id}'"),
            reads,
            commands,
        );
        return;
    };
    if reads.requested.contains(id) {
        if !retry && !reads.failed.remove(id) {
            return;
        }
        if retry {
            reads.failed.remove(id);
            reads.delivered.remove(id);
        }
    } else {
        reads.requested.insert(id.to_owned());
    }
    let Some(asset_server) = asset_server else {
        report_dataset_request_failure(
            id,
            "the shared asset server is unavailable".to_owned(),
            reads,
            commands,
        );
        return;
    };

    let asset_uri = entry.artifact_uri();
    let handle = asset_server.load::<TextAsset>(asset_uri.clone());
    if let Some(asset) = assets.and_then(|assets| assets.get(&handle)) {
        reads.delivered.insert(id.to_owned());
        reads.failed.remove(id);
        commands.trigger(DatasetTextArtifactReady {
            id: id.to_owned(),
            text: asset.text.clone(),
        });
        return;
    }

    let pending = PendingDatasetArtifactRead {
        id: id.to_owned(),
        scene_generation: reads.scene_generation,
        asset_uri,
        _handle: handle,
    };
    let requests = reads.handles.entry(pending._handle.id()).or_default();
    if let Some(existing) = requests.iter_mut().find(|existing| {
        existing.id == pending.id && existing.scene_generation == pending.scene_generation
    }) {
        *existing = pending;
    } else {
        requests.push(pending);
    }
}

fn publish_dataset_artifact_reads(
    mut changes: MessageReader<AssetEvent<TextAsset>>,
    mut failures: MessageReader<AssetLoadFailedEvent<TextAsset>>,
    assets: Option<Res<Assets<TextAsset>>>,
    reads: ResMut<DatasetArtifactReads>,
    mut commands: Commands,
) {
    enum Completion {
        Ready(String, String, u64),
        Failed(String, String, u64),
    }

    let mut completed = Vec::new();
    for change in changes.read() {
        let id = match change {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            AssetEvent::LoadedWithDependencies { .. } => continue,
            AssetEvent::Removed { .. } | AssetEvent::Unused { .. } => continue,
        };
        let Some(requests) = reads.handles.get(&id) else {
            continue;
        };
        let text = assets
            .as_deref()
            .and_then(|assets| assets.get(id))
            .map(|asset| asset.text.clone());
        for request in requests {
            if request.scene_generation != reads.scene_generation
                || !reads.requested.contains(&request.id)
                || reads.delivered.contains(&request.id)
            {
                continue;
            }
            completed.push(match &text {
                Some(text) => {
                    Completion::Ready(request.id.clone(), text.clone(), request.scene_generation)
                }
                None => Completion::Failed(
                    request.id.clone(),
                    format!("asset event for '{}' had no loaded text", request.asset_uri),
                    request.scene_generation,
                ),
            });
        }
    }

    for failure in failures.read() {
        let Some(requests) = reads.handles.get(&failure.id) else {
            continue;
        };
        for request in requests {
            if request.scene_generation == reads.scene_generation
                && reads.requested.contains(&request.id)
                && !reads.delivered.contains(&request.id)
            {
                completed.push(Completion::Failed(
                    request.id.clone(),
                    failure.error.to_string(),
                    request.scene_generation,
                ));
            }
        }
    }

    for completion in completed {
        let (id, generation) = match &completion {
            Completion::Ready(id, _, generation) | Completion::Failed(id, _, generation) => {
                (id.clone(), *generation)
            }
        };
        commands.queue(move |world: &mut World| {
            let Some(mut reads) = world.get_resource_mut::<DatasetArtifactReads>() else {
                return;
            };
            if reads.scene_generation != generation || !reads.requested.contains(&id) {
                return;
            }
            match completion {
                Completion::Ready(id, text, _) => {
                    if reads.delivered.insert(id.clone()) {
                        reads.failed.remove(&id);
                        drop(reads);
                        world.trigger(DatasetTextArtifactReady { id, text });
                    }
                }
                Completion::Failed(id, error, _) => {
                    if reads.failed.insert(id.clone()) {
                        drop(reads);
                        report_dataset_asset_failure(world, &id, &error);
                    }
                }
            }
        });
    }
}

fn report_dataset_request_failure(
    id: &str,
    detail: String,
    reads: &mut DatasetArtifactReads,
    commands: &mut Commands,
) {
    if reads.failed.insert(id.to_owned()) {
        commands.trigger(lunco_assets_datasets::dataset_failed(format!(
            "scene asset policy requested dataset '{id}', but {detail}"
        )));
    }
}

fn report_dataset_asset_failure(world: &mut World, id: &str, error: &str) {
    let state = world
        .get_resource::<lunco_assets_datasets::DatasetRegistry>()
        .and_then(|registry| registry.entry(id))
        .map(|entry| entry.state.clone());
    if matches!(
        state,
        Some(
            lunco_assets_datasets::DatasetState::Downloading { .. }
                | lunco_assets_datasets::DatasetState::Processing { .. }
                | lunco_assets_datasets::DatasetState::Cancelling
        )
    ) {
        if let Some(mut reads) = world.get_resource_mut::<DatasetArtifactReads>() {
            reads.failed.remove(id);
        }
        return;
    }
    let state_text = state.map_or_else(
        || "dataset state unavailable".to_owned(),
        |state| format!("dataset state is {state:?}"),
    );
    world.trigger(lunco_assets_datasets::dataset_failed(format!(
        "scene requires dataset '{id}', but its canonical text asset could not be loaded ({state_text}): {error}"
    )));
}
