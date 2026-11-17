//! Icon assets use the existing Modelica loader and source-revision facts.
use super::{UsdCanvasSessionState, UsdCanvasState, UsdPrimNodeData};
use bevy::prelude::*;
use lunco_modelica_ast::annotations::Icon;
use lunco_modelica_runtime::ModelicaSource;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub(super) struct Icons {
    entries: BTreeMap<String, Entry>,
    pending: bool,
    ready: BTreeMap<String, Arc<Icon>>,
}
struct Entry {
    source: String,
    class: Option<String>,
    handle: Handle<ModelicaSource>,
}

pub(super) fn prepare(
    state: &mut UsdCanvasSessionState,
    server: &AssetServer,
    roots: Option<&lunco_assets_core::TwinRoots>,
    prepared: Option<&lunco_assets_core::asset_path::PreparedAssetPaths>,
) {
    let Some(stage) = state.stage_id else {
        return;
    };
    let candidates: BTreeMap<_, _> = state
        .source_nodes
        .iter()
        .flat_map(|node| node.programs.iter())
        .filter(|p| p.backend == "Modelica" && p.issue.is_none())
        .map(|p| (p.path.clone(), (p.source.clone(), p.modelica_class.clone())))
        .collect();
    state
        .icons
        .entries
        .retain(|path, _| candidates.contains_key(path));
    state
        .icons
        .ready
        .retain(|path, _| candidates.contains_key(path));
    for (path, (source, class)) in candidates {
        if state
            .icons
            .entries
            .get(&path)
            .is_some_and(|entry| entry.source == source && entry.class == class)
        {
            continue;
        }
        // Retire the previous source even if the replacement cannot load.
        state.icons.entries.remove(&path);
        state.icons.ready.remove(&path);
        state.icons.pending = true;
        match lunco_usd_bevy_stage::asset::resolve_stage_asset_path(
            server, stage, &source, roots, prepared,
        ) {
            Ok(asset) => {
                state.icons.entries.insert(
                    path,
                    Entry {
                        source,
                        class,
                        handle: server.load(asset),
                    },
                );
            }
            Err(error) => {
                state.last_error = Some(format!("Modelica icon source {source}: {error}"))
            }
        }
    }
}

pub(super) fn refresh(
    mut events: MessageReader<AssetEvent<ModelicaSource>>,
    sources: Res<Assets<ModelicaSource>>,
    mut views: ResMut<UsdCanvasState>,
) {
    let changed = events.read().count() > 0;
    let views = &mut *views;
    for state in std::iter::once(&mut views.scene).chain(views.sessions.values_mut()) {
        if !changed && !state.icons.pending {
            continue;
        }
        state.icons.pending = false;
        for (path, entry) in &state.icons.entries {
            let icon = sources.get(&entry.handle).and_then(|source| {
                let class = entry.class.clone().or_else(|| {
                    source.interface.model_name.as_ref().map(|name| {
                        lunco_modelica_ast::ast_extract::qualify(
                            source.interface.within.as_deref().unwrap_or(""),
                            name,
                        )
                    })
                })?;
                source.interface.icons.get(&class)
            });
            if let Some(icon) = icon {
                state
                    .icons
                    .ready
                    .insert(path.clone(), Arc::new(icon.clone()));
            } else {
                state.icons.ready.remove(path);
            }
        }
        apply(state);
    }
}

pub(super) fn apply(state: &mut UsdCanvasSessionState) {
    let ready = &state.icons.ready;
    for scene in std::iter::once(&mut state.canvas.scene).chain(state.expanded_scene.iter_mut()) {
        let ids: Vec<_> = scene.nodes().map(|(id, _)| *id).collect();
        for id in ids {
            let Some(node) = scene.node_mut(id) else {
                continue;
            };
            let Some(data) = node.data.downcast_ref::<UsdPrimNodeData>() else {
                continue;
            };
            let icon = data
                .icon_program
                .as_ref()
                .and_then(|program| ready.get(program));
            if data.modelica_icon.as_deref() == icon.map(Arc::as_ref) {
                continue;
            }
            let mut data = data.clone();
            data.modelica_icon = icon.cloned();
            node.data = Arc::new(data);
        }
    }
}
