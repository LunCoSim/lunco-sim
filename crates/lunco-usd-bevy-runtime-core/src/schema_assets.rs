//! Runtime loading of the USD schema sources used by authored-data helpers.
//!
//! The schema registry owns parsing and lookup. This module owns only the Bevy
//! lifecycle that supplies schema text from the normal USD asset pipeline,
//! including on wasm. Each source is admitted independently so one damaged
//! optional module does not prevent valid modules from becoming available; every
//! failed source is reported explicitly. Cross-schema linear-unit validation
//! waits until every vendored core source has finished loading.

use std::collections::HashSet;

use bevy::asset::{AssetId, AssetServer, Assets};
use bevy::prelude::*;
use lunco_usd_bevy_stage::source::UsdSourceText;

#[derive(Resource)]
pub(crate) struct PendingSchemaAssets {
    entries: Vec<(bool, String, Handle<UsdSourceText>)>,
    core_sources: Vec<AssetId<UsdSourceText>>,
    finished: HashSet<AssetId<UsdSourceText>>,
    failed: HashSet<AssetId<UsdSourceText>>,
    core_linear_units_validated: bool,
    complete: bool,
}

/// Request the runtime schema sources once all asset loaders have been built.
pub(crate) fn request_schema_assets(
    mut commands: Commands,
    asset_server: Option<Res<AssetServer>>,
    manifest: Option<Res<lunco_assets_runtime::discovery::AssetManifest>>,
    pending: Option<Res<PendingSchemaAssets>>,
) {
    if pending.is_some() {
        return;
    }
    let Some(manifest) = manifest else {
        return;
    };
    if !manifest.ready() {
        return;
    }
    let Some(asset_server) = asset_server else {
        error!("[schema] USD schema loading is unavailable: AssetServer is not installed");
        return;
    };

    let mut entries = manifest
        .rels()
        .iter()
        .filter_map(|path| {
            let owner = lunco_assets_runtime::discovery::schema_asset_owner(path)?;
            let path = std::path::Path::new(path);
            let module = path.file_stem()?.to_str()?.to_owned();
            Some((
                owner == lunco_assets_runtime::discovery::SchemaAssetOwner::Lunco,
                module,
                asset_server.load::<UsdSourceText>(path.to_string_lossy().into_owned()),
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.1.cmp(&right.1));
    let core_sources = entries
        .iter()
        .filter_map(|(own, _, handle)| (!own).then_some(handle.id()))
        .collect();
    if entries.is_empty() {
        error!("[schema] runtime asset manifest contains no USDA schema sources");
    }
    commands.insert_resource(PendingSchemaAssets {
        entries,
        core_sources,
        finished: HashSet::default(),
        failed: HashSet::default(),
        core_linear_units_validated: false,
        complete: false,
    });
}

/// Register each schema source as soon as its text is available.
pub(crate) fn register_ready_schema_assets(
    pending: Option<ResMut<PendingSchemaAssets>>,
    assets: Option<Res<Assets<UsdSourceText>>>,
    asset_server: Option<Res<AssetServer>>,
) {
    let (Some(pending), Some(assets), Some(asset_server)) = (pending, assets, asset_server) else {
        return;
    };
    let PendingSchemaAssets {
        entries,
        core_sources,
        finished,
        failed,
        core_linear_units_validated,
        complete,
    } = pending.into_inner();

    for (own, module, handle) in entries.iter() {
        let id = handle.id();
        if finished.contains(&id) || failed.contains(&id) {
            continue;
        }

        if let Some(source) = assets.get(handle.id()) {
            let registered = if *own {
                lunco_usd_authoring::schema::SchemaRegistry::register_extension(&source.0)
            } else {
                lunco_usd_authoring::schema::SchemaRegistry::register_core_extension(&source.0)
            };
            if registered {
                info!("[schema] loaded {module} schema asset");
                finished.insert(id);
            } else {
                error!("[schema] rejected invalid {module} schema asset");
                failed.insert(id);
            }
            continue;
        }

        if asset_server
            .get_load_state(id)
            .is_some_and(|state| state.is_failed())
        {
            error!("[schema] failed to load {module} schema asset");
            failed.insert(id);
        }
    }

    if !*core_linear_units_validated {
        let core_sources_settled = !core_sources.is_empty()
            && core_sources
                .iter()
                .all(|id| finished.contains(id) || failed.contains(id));
        if core_sources_settled {
            if core_sources.iter().all(|id| finished.contains(id)) {
                lunco_usd_authoring::schema::SchemaRegistry::validate_core_linear_units();
            }
            *core_linear_units_validated = true;
        }
    }

    *complete = entries.iter().all(|(_, _, handle)| {
        let id = handle.id();
        finished.contains(&id) || failed.contains(&id)
    }) && (core_sources.is_empty() || *core_linear_units_validated);
}

/// Keep the per-frame registration system asleep once every requested schema
/// source has reached a terminal load state and core unit validation has run.
pub(crate) fn schema_registration_pending(pending: Option<Res<PendingSchemaAssets>>) -> bool {
    pending.is_some_and(|pending| !pending.complete)
}
