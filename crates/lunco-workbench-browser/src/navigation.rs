//! Browser navigation uses the existing file inventory and document dispatch.

use crate::{BrowserAction, BrowserActions, BrowserQuery, BrowserScope, TWIN_BROWSER_PANEL_ID};
use bevy::prelude::*;
use lunco_doc::{Ack, OpId};
use lunco_workbench_core::commands::{FocusPanel, RevealNavigationSource};
use lunco_workbench_core::navigation::{NavigationEntry, NavigationRegistry};
use lunco_workbench_core::source::{OpenSourceView, OpenTwinSource};
use std::sync::Arc;

#[derive(Resource, Clone, Debug)]
pub struct BrowserView {
    pub scope: BrowserScope,
}

impl Default for BrowserView {
    fn default() -> Self {
        Self {
            scope: BrowserScope::Models,
        }
    }
}

/// Select a navigator lens while preserving the shared search query.
#[lunco_core::Command(default)]
pub struct SetBrowserScope {
    pub scope: String,
}

/// Open a file from the active Twin through the same route as its browser row.
#[lunco_core::Command(default)]
pub struct OpenBrowserFile {
    pub relative_path: String,
}

#[lunco_core::on_command(SetBrowserScope)]
fn set_scope(trigger: On<SetBrowserScope>, mut view: ResMut<BrowserView>) -> Result<Ack, String> {
    view.scope = match trigger.scope.as_str() {
        "models" => BrowserScope::Models,
        "files" => BrowserScope::Files,
        "libraries" => BrowserScope::Libraries,
        _ => return Err("Unknown browser scope; expected models, files, or libraries".into()),
    };
    Ok(Ack::new(OpId::new()))
}

#[lunco_core::on_command(OpenBrowserFile)]
fn open_file(
    trigger: On<OpenBrowserFile>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    kinds: Option<Res<lunco_twin::DocumentKindRegistry>>,
    mut actions: ResMut<BrowserActions>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let twin = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .ok_or("No active Twin")?;
    let path = std::path::Path::new(&trigger.relative_path);
    if !lunco_twin::is_safe_relative_path(path)
        || !twin.files().iter().any(|file| file.relative_path == path)
    {
        return Err("File is not in the current Twin inventory".into());
    }
    if crate::twin_browser::files_section::should_open_as_document(path, kinds.as_deref(), true) {
        actions.push(BrowserAction::OpenFile {
            relative_path: twin.root.join(path),
        });
    } else {
        commands.trigger(OpenTwinSource {
            twin_root: twin.root.display().to_string(),
            relative_path: trigger.relative_path.clone(),
            pinned: true,
            focus: None,
        });
    }
    Ok(Ack::new(OpId::new()))
}

#[lunco_core::on_command(RevealNavigationSource)]
fn reveal(
    trigger: On<RevealNavigationSource>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut query: ResMut<BrowserQuery>,
    mut view: ResMut<BrowserView>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let uri = &trigger.uri;
    if let Some(relative) = workspace
        .active_twin
        .and_then(|id| workspace.twin(id))
        .and_then(|twin| std::path::Path::new(uri).strip_prefix(&twin.root).ok())
    {
        view.scope = BrowserScope::Files;
        query.text = relative.display().to_string();
    } else if let Some(relative) = uri.strip_prefix("lunco://") {
        view.scope = BrowserScope::Libraries;
        query.text = relative.to_owned();
    } else {
        return Err("This source is outside the active Twin and bundled library; its exact path remains in the context bar".into());
    }
    commands.trigger(FocusPanel {
        id: TWIN_BROWSER_PANEL_ID.as_str().into(),
    });
    Ok(Ack::new(OpId::new()))
}

lunco_core::register_commands!(set_scope, open_file, reveal);

fn twin_entries(world: &World) -> Result<Vec<NavigationEntry>, String> {
    let mut entries = Vec::new();
    if let Some(workspace) = world.get_resource::<lunco_workspace::WorkspaceResource>() {
        if let Some((id, twin)) = workspace
            .active_twin
            .and_then(|id| workspace.twin(id).map(|twin| (id, twin)))
        {
            for file in twin.files() {
                let path = file.relative_path.display().to_string();
                let target = path.clone();
                entries.push(NavigationEntry {
                    id: format!("twin:{}:{path}", id.raw()),
                    label: file
                        .relative_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    detail: format!(
                        "{} / {path}",
                        twin.manifest
                            .as_ref()
                            .map(|manifest| manifest.name.as_str())
                            .unwrap_or("Twin")
                    ),
                    category: "Twin".into(),
                    activate: Arc::new(move |ctx| {
                        ctx.trigger(OpenBrowserFile {
                            relative_path: target.clone(),
                        })
                    }),
                });
            }
        }
    }
    Ok(entries)
}

fn library_entries(world: &World) -> Result<Vec<NavigationEntry>, String> {
    let mut entries = Vec::new();
    let manifest = world
        .get_resource::<lunco_assets_runtime::discovery::AssetManifest>()
        .ok_or("Library inventory unavailable")?;
    if !manifest.ready() {
        return Err("Library inventory is loading".into());
    }
    for asset in lunco_assets_runtime::discovery::list_library_assets(manifest)
        .map_err(|error| error.to_string())?
    {
        let source = asset.asset_path.clone();
        entries.push(NavigationEntry {
            id: format!("library:{source}"),
            label: std::path::Path::new(&asset.rel)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            detail: format!("LunCo Library / {} · read-only", asset.rel),
            category: "Libraries".into(),
            activate: Arc::new(move |ctx| {
                ctx.trigger(OpenSourceView {
                    asset_path: source.clone(),
                })
            }),
        });
    }
    Ok(entries)
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<BrowserView>()
        .init_resource::<NavigationRegistry>();
    app.world_mut()
        .resource_mut::<NavigationRegistry>()
        .register(&["Twin"], twin_entries);
    app.world_mut()
        .resource_mut::<NavigationRegistry>()
        .register(&["Libraries"], library_entries);
    app.init_resource::<lunco_workbench_core::WorkbenchMenuRegistry>();
    app.world_mut()
        .resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
        .register_context_bar(|ui, ctx| {
            if ui.button("Browse Twin").clicked() {
                ctx.trigger(FocusPanel {
                    id: TWIN_BROWSER_PANEL_ID.as_str().into(),
                });
            }
        });
    register_all_commands(app);
}
