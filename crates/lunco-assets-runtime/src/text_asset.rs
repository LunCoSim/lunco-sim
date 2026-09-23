//! Generic UTF-8 text assets for small runtime-authored catalogs and policies.
//!
//! The asset type is intentionally language-neutral. Modelica and Rhai keep
//! their richer loaders, while small catalogs and text datasets can use one
//! asynchronous Bevy path on native and wasm instead of synchronous file reads
//! or compiled-in snapshots.

use bevy::asset::{AssetEvent, AssetLoadFailedEvent, AssetLoader, LoadContext, io::Reader};
use bevy::prelude::*;
use std::collections::HashMap;

use crate::discovery::AssetManifest;

/// UTF-8 text loaded from the runtime asset tree.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct TextAsset {
    /// The decoded text.
    pub text: String,
}

/// Loader for UTF-8 text assets.
#[derive(Default, TypePath)]
pub struct TextAssetLoader;

impl AssetLoader for TextAssetLoader {
    type Asset = TextAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(TextAsset {
            text: String::from_utf8(bytes)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["json", "toml", "csv"]
    }
}

/// One text asset discovered from the engine library or an open Twin.
#[derive(Clone, Debug)]
pub struct TextAssetEntry {
    /// The logical asset path used with [`AssetServer::load`].
    pub asset_path: String,
    /// Workspace Twin identity for Twin-owned content.
    pub twin_id: Option<lunco_workspace::TwinId>,
    /// Exact asset source authority assigned to a Twin.
    pub twin_name: Option<String>,
    /// The handle for the asynchronously loaded text.
    pub handle: Handle<TextAsset>,
}

/// One JSON source in a complete asynchronous asset-scope snapshot.
#[derive(Clone, Debug)]
pub struct JsonAssetRecord {
    /// Canonical `lunco://` or `twin://` source address.
    pub asset_uri: String,
    /// Current text, or `None` with `error` when loading failed.
    pub text: Option<String>,
    /// Asset-owner error for an unreadable source.
    pub error: Option<String>,
}

/// JSON asset enumeration began for an application or Twin scope.
#[derive(Event, Clone, Debug)]
pub struct JsonAssetScopeLoading {
    /// Workspace Twin identity, or `None` for the engine asset library.
    pub twin_id: Option<lunco_workspace::TwinId>,
    /// Assigned Twin asset authority when the scope is Twin-owned.
    pub twin_name: Option<String>,
    /// Canonical URI root used to qualify references owned by this scope.
    pub asset_root_uri: String,
    /// Number of JSON assets whose asynchronous read must settle.
    pub asset_count: usize,
}

/// A complete JSON asset snapshot is ready or has changed.
#[derive(Event, Clone, Debug)]
pub struct JsonAssetScopeChanged {
    /// Workspace Twin identity, or `None` for the engine asset library.
    pub twin_id: Option<lunco_workspace::TwinId>,
    /// Assigned Twin asset authority when the scope is Twin-owned.
    pub twin_name: Option<String>,
    /// Canonical URI root used to qualify references owned by this scope.
    pub asset_root_uri: String,
    /// Every indexed JSON source in this scope and its read result.
    pub assets: Vec<JsonAssetRecord>,
}

/// Runtime catalog of small text assets in the engine library and open Twins.
///
/// This catalog answers only “which text assets exist?” and starts their
/// platform-neutral loads. Consumers inspect the loaded text and own their
/// semantic kind/schema, so this layer remains independent of domain and policy
/// types.
#[derive(Resource, Default)]
pub struct TextAssetCatalog {
    entries: Vec<TextAssetEntry>,
    ready: bool,
}

#[derive(Resource, Default)]
struct TextAssetReadiness {
    terminal: HashMap<AssetId<TextAsset>, Result<(), String>>,
}

impl TextAssetCatalog {
    /// Whether the runtime manifest has been enumerated.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// All discovered JSON/TOML catalog assets in the engine library and open Twins.
    pub fn entries(&self) -> &[TextAssetEntry] {
        &self.entries
    }
}

fn discover_text_assets(
    mut catalog: ResMut<TextAssetCatalog>,
    manifest: Option<Res<AssetManifest>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<TextAsset>>>,
    mut readiness: ResMut<TextAssetReadiness>,
    mut commands: Commands,
) {
    if catalog.ready {
        return;
    }
    let (Some(manifest), Some(asset_server)) = (manifest, asset_server) else {
        return;
    };
    if !manifest.ready() {
        return;
    }

    let mut entries = catalog
        .entries
        .drain(..)
        .filter(|entry| entry.twin_id.is_some())
        .collect::<Vec<_>>();
    entries.extend(
        manifest
            .rels()
            .iter()
            .filter(|path| {
                matches!(
                    std::path::Path::new(path)
                        .extension()
                        .and_then(|ext| ext.to_str()),
                    Some("json" | "toml")
                )
            })
            .map(|asset_path| TextAssetEntry {
                asset_path: asset_path.clone(),
                twin_id: None,
                twin_name: None,
                handle: asset_server.load(asset_path.clone()),
            })
            .collect::<Vec<_>>(),
    );
    catalog.entries = entries;
    catalog.ready = true;
    if let Some(assets) = assets.as_deref() {
        seed_ready_assets(
            &catalog.entries,
            None,
            &asset_server,
            assets,
            &mut readiness,
        );
    }
    let json_count = json_entries(&catalog.entries, None).count();
    let asset_root_uri = lunco_assets_core::engine_asset_uri("");
    commands.trigger(JsonAssetScopeLoading {
        twin_id: None,
        twin_name: None,
        asset_root_uri: asset_root_uri.clone(),
        asset_count: json_count,
    });
    publish_json_scope_if_ready(
        None,
        None,
        asset_root_uri,
        &catalog,
        &readiness,
        assets.as_deref(),
        &mut commands,
    );
}

fn load_twin_text_assets(
    trigger: On<crate::TwinAssetMounted>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<TextAsset>>>,
    mut catalog: ResMut<TextAssetCatalog>,
    mut readiness: ResMut<TextAssetReadiness>,
    mut commands: Commands,
) {
    let Some(twin) = workspace
        .as_deref()
        .and_then(|workspace| workspace.twin(trigger.event().twin))
    else {
        return;
    };
    let twin_id = trigger.event().twin;
    readiness.terminal.retain(|id, _| {
        !catalog
            .entries
            .iter()
            .any(|entry| entry.twin_id == Some(twin_id) && entry.handle.id() == *id)
    });
    catalog
        .entries
        .retain(|entry| entry.twin_id != Some(twin_id));
    let Some(asset_server) = asset_server else {
        return;
    };
    for file in twin.files() {
        let extension = file
            .relative_path
            .extension()
            .and_then(|extension| extension.to_str());
        if !matches!(extension, Some("json" | "toml")) {
            continue;
        }
        let asset_path = lunco_assets_core::twin_uri(&trigger.event().name, &file.relative_path);
        let handle = asset_server.load::<TextAsset>(asset_path.clone());
        catalog.entries.push(TextAssetEntry {
            asset_path,
            twin_id: Some(twin_id),
            twin_name: Some(trigger.event().name.clone()),
            handle,
        });
    }
    if let Some(assets) = assets.as_deref() {
        seed_ready_assets(
            &catalog.entries,
            Some(twin_id),
            &asset_server,
            assets,
            &mut readiness,
        );
    }
    let json_count = json_entries(&catalog.entries, Some(twin_id)).count();
    let asset_root_uri = lunco_assets_core::twin_uri(&trigger.event().name, "");
    commands.trigger(JsonAssetScopeLoading {
        twin_id: Some(twin_id),
        twin_name: Some(trigger.event().name.clone()),
        asset_root_uri: asset_root_uri.clone(),
        asset_count: json_count,
    });
    publish_json_scope_if_ready(
        Some(twin_id),
        Some(trigger.event().name.clone()),
        asset_root_uri,
        &catalog,
        &readiness,
        assets.as_deref(),
        &mut commands,
    );
}

fn release_twin_text_assets(
    trigger: On<lunco_workspace::TwinClosed>,
    mut catalog: ResMut<TextAssetCatalog>,
    mut readiness: ResMut<TextAssetReadiness>,
) {
    let twin_id = trigger.event().twin;
    let closed_ids = catalog
        .entries
        .iter()
        .filter(|entry| entry.twin_id == Some(twin_id))
        .map(|entry| entry.handle.id())
        .collect::<Vec<_>>();
    catalog
        .entries
        .retain(|entry| entry.twin_id != Some(twin_id));
    readiness.terminal.retain(|id, _| !closed_ids.contains(id));
}

fn seed_ready_assets(
    entries: &[TextAssetEntry],
    twin_id: Option<lunco_workspace::TwinId>,
    asset_server: &AssetServer,
    assets: &Assets<TextAsset>,
    readiness: &mut TextAssetReadiness,
) {
    for entry in json_entries(entries, twin_id) {
        let id = entry.handle.id();
        if assets.get(id).is_some() {
            readiness.terminal.insert(id, Ok(()));
        } else if asset_server
            .get_load_state(id)
            .is_some_and(|state| state.is_failed())
        {
            readiness
                .terminal
                .insert(id, Err("the asset had already failed to load".to_owned()));
        }
    }
}

fn json_entries(
    entries: &[TextAssetEntry],
    twin_id: Option<lunco_workspace::TwinId>,
) -> impl Iterator<Item = &TextAssetEntry> {
    entries.iter().filter(move |entry| {
        entry.twin_id == twin_id
            && std::path::Path::new(&entry.asset_path)
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("json")
    })
}

fn json_asset_uri(entry: &TextAssetEntry) -> String {
    if entry.twin_id.is_some() {
        entry.asset_path.clone()
    } else {
        lunco_assets_core::engine_asset_uri(&entry.asset_path)
    }
}

fn publish_json_scope_if_ready(
    twin_id: Option<lunco_workspace::TwinId>,
    twin_name: Option<String>,
    asset_root_uri: String,
    catalog: &TextAssetCatalog,
    readiness: &TextAssetReadiness,
    assets: Option<&Assets<TextAsset>>,
    commands: &mut Commands,
) {
    let entries = json_entries(&catalog.entries, twin_id).collect::<Vec<_>>();
    if entries
        .iter()
        .any(|entry| !readiness.terminal.contains_key(&entry.handle.id()))
    {
        return;
    }
    let records = entries
        .iter()
        .map(|entry| {
            let asset_uri = json_asset_uri(entry);
            match readiness.terminal.get(&entry.handle.id()) {
                Some(Ok(())) => match assets.and_then(|assets| assets.get(&entry.handle)) {
                    Some(asset) => JsonAssetRecord {
                        asset_uri,
                        text: Some(asset.text.clone()),
                        error: None,
                    },
                    None => JsonAssetRecord {
                        asset_uri,
                        text: None,
                        error: Some("the asset was marked loaded but has no text".into()),
                    },
                },
                Some(Err(error)) => JsonAssetRecord {
                    asset_uri,
                    text: None,
                    error: Some(error.clone()),
                },
                None => JsonAssetRecord {
                    asset_uri,
                    text: None,
                    error: Some("asset readiness changed while building the snapshot".into()),
                },
            }
        })
        .collect();
    commands.trigger(JsonAssetScopeChanged {
        twin_id,
        twin_name,
        asset_root_uri,
        assets: records,
    });
}

fn publish_json_asset_changes(
    mut changes: MessageReader<AssetEvent<TextAsset>>,
    mut failures: MessageReader<AssetLoadFailedEvent<TextAsset>>,
    catalog: Res<TextAssetCatalog>,
    assets: Option<Res<Assets<TextAsset>>>,
    mut readiness: ResMut<TextAssetReadiness>,
    mut commands: Commands,
) {
    let mut dirty_scopes = Vec::new();
    for change in changes.read() {
        let id = match change {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            AssetEvent::LoadedWithDependencies { .. } => continue,
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                let Some(entry) = catalog
                    .entries
                    .iter()
                    .find(|entry| entry.handle.id() == *id)
                else {
                    continue;
                };
                if entry.asset_path.ends_with(".json") {
                    readiness.terminal.insert(
                        *id,
                        Err("the asset was removed before its owning scope closed".into()),
                    );
                    dirty_scopes.push(entry.twin_id);
                }
                continue;
            }
        };
        let Some(entry) = catalog.entries.iter().find(|entry| entry.handle.id() == id) else {
            continue;
        };
        if !entry.asset_path.ends_with(".json") {
            continue;
        }
        let terminal = if assets
            .as_deref()
            .is_some_and(|assets| assets.get(id).is_some())
        {
            Ok(())
        } else {
            Err("the asset event arrived without loaded text".to_owned())
        };
        readiness.terminal.insert(id, terminal);
        dirty_scopes.push(entry.twin_id);
    }
    for failure in failures.read() {
        let Some(entry) = catalog
            .entries
            .iter()
            .find(|entry| entry.handle.id() == failure.id)
        else {
            continue;
        };
        if entry.asset_path.ends_with(".json") {
            readiness
                .terminal
                .insert(failure.id, Err(failure.error.to_string()));
            dirty_scopes.push(entry.twin_id);
        }
    }
    dirty_scopes.sort_by_key(|scope| scope.map(lunco_workspace::TwinId::raw));
    dirty_scopes.dedup();
    for twin_id in dirty_scopes {
        let twin_name = twin_id.and_then(|twin_id| {
            catalog
                .entries
                .iter()
                .find(|entry| entry.twin_id == Some(twin_id))
                .and_then(|entry| entry.twin_name.clone())
        });
        let asset_root_uri = match (twin_id, twin_name.as_deref()) {
            (None, None) => lunco_assets_core::engine_asset_uri(""),
            (Some(_), Some(name)) if !name.is_empty() => lunco_assets_core::twin_uri(name, ""),
            _ => {
                warn!("[assets] cannot publish JSON snapshot without its asset scope authority");
                continue;
            }
        };
        publish_json_scope_if_ready(
            twin_id,
            twin_name,
            asset_root_uri,
            &catalog,
            &readiness,
            assets.as_deref(),
            &mut commands,
        );
    }
}

/// Registers the generic text asset type.
pub struct TextAssetPlugin;

impl Plugin for TextAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<TextAsset>()
            .init_asset_loader::<TextAssetLoader>()
            .init_resource::<TextAssetCatalog>()
            .init_resource::<TextAssetReadiness>()
            .add_observer(load_twin_text_assets)
            .add_observer(release_twin_text_assets)
            .add_systems(
                Update,
                (discover_text_assets, publish_json_asset_changes).chain(),
            );
    }
}
