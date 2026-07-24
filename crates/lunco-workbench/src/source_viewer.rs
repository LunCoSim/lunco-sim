//! Domain-neutral text editor for files that do not have a richer editor.

use std::path::{Component, Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use bevy_egui::egui;
use lunco_core::on_command;
use lunco_doc_bevy::OpenFile;

use crate::{
    CloseTab, EditorTabId, EditorTabs, InstancePanel, OpenSourceView, OpenTab, OpenTwinSource,
    PanelCtx, PanelId, PanelSlot, PendingTabCloses, SaveSourceText, TabId,
};

const SOURCE_EDITOR_KIND: PanelId = PanelId("source_editor");
const TEXT_VIEW_EXTS: &[&str] = &["rhai", "btxml", "wgsl"];

pub(crate) struct SourceEditorPanel;

impl InstancePanel for SourceEditorPanel {
    fn kind(&self) -> PanelId {
        SOURCE_EDITOR_KIND
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn title(&self, world: &World, instance: u64) -> String {
        world
            .get_resource::<EditorTabs<SourceTabState>>()
            .and_then(|tabs| tabs.get(instance))
            .map(|tab| {
                let name = tab
                    .state
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| tab.state.path.display().to_string());
                if tab.state.dirty {
                    format!("● {name}")
                } else if tab.pinned {
                    name
                } else {
                    format!("{name} (Preview)")
                }
            })
            .unwrap_or_else(|| "Source".into())
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        let mut save = None;
        let error_color = ctx.resource_expect::<lunco_theme::Theme>().tokens.error;
        let rendered = ctx
            .resource_scope::<EditorTabs<SourceTabState>, _>(|_, tabs| {
                let Some(tab) = tabs.get_mut(instance) else {
                    return false;
                };
                let source = &mut tab.state;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(source.path.display().to_string()).weak());
                    if let Some(origin) = &source.origin {
                        for (label, update) in [("Save", false), ("Save & Update", true)] {
                            if ui
                                .add_enabled(!source.saving, egui::Button::new(label))
                                .clicked()
                            {
                                source.saving = true;
                                save = Some(SaveSourceText {
                                    twin_root: origin.twin_root.to_string_lossy().into_owned(),
                                    relative_path: origin
                                        .relative_path
                                        .to_string_lossy()
                                        .into_owned(),
                                    text: source.text.clone(),
                                    update,
                                });
                            }
                        }
                    } else {
                        ui.label(egui::RichText::new("(read-only)").weak());
                    }
                    if source.loading || source.saving {
                        ui.spinner();
                    }
                });
                ui.separator();
                if let Some(error) = &source.error {
                    ui.colored_label(error_color, error);
                }
                if source.loading {
                    ui.label(egui::RichText::new("Loading…").weak().italics());
                } else if source.origin.is_some() {
                    let response = egui::ScrollArea::both()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut source.text)
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .desired_width(f32::INFINITY),
                            )
                        })
                        .inner;
                    source.dirty |= response.changed();
                } else {
                    egui::ScrollArea::both()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut source.text.as_str())
                                    .font(egui::TextStyle::Monospace)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );
                        });
                }
                true
            })
            .unwrap_or(false);
        if let Some(command) = save {
            ctx.trigger(command);
        }
        if !rendered {
            ui.label("This editor tab is no longer available.");
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourceTabState {
    path: PathBuf,
    text: String,
    origin: Option<TwinSourceOrigin>,
    dirty: bool,
    loading: bool,
    saving: bool,
    error: Option<String>,
    request: u64,
}

#[derive(Debug, Clone)]
struct TwinSourceOrigin {
    twin_root: PathBuf,
    relative_path: PathBuf,
}

#[derive(Resource, Default)]
pub(crate) struct PendingSourceRequests {
    opens: Vec<SourceOpenRequest>,
    saves: Vec<SaveSourceText>,
}

enum SourceOpenRequest {
    Path(String),
    Asset(String),
    Twin {
        root: PathBuf,
        relative: PathBuf,
        pinned: bool,
    },
}

#[derive(Resource, Default)]
pub(crate) struct PendingSourceReads {
    next_request: u64,
    tasks: Vec<PendingSourceRead>,
}

struct PendingSourceRead {
    tab: EditorTabId,
    request: u64,
    #[cfg(not(target_arch = "wasm32"))]
    task: Task<Result<String, String>>,
    #[cfg(target_arch = "wasm32")]
    result: crossbeam_channel::Receiver<Result<String, String>>,
}

#[derive(Resource, Default)]
pub(crate) struct PendingSourceWrites {
    tasks: Vec<PendingSourceWrite>,
}

struct PendingSourceWrite {
    tab: EditorTabId,
    path: PathBuf,
    saved_text: String,
    update: bool,
    #[cfg(not(target_arch = "wasm32"))]
    task: Task<Result<(), String>>,
    #[cfg(target_arch = "wasm32")]
    result: crossbeam_channel::Receiver<Result<(), String>>,
}

#[on_command(OpenFile)]
pub(crate) fn on_open_file_for_text(
    trigger: On<OpenFile>,
    mut pending: ResMut<PendingSourceRequests>,
) {
    let path = trigger.event().path.clone();
    if Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| TEXT_VIEW_EXTS.contains(&extension))
    {
        pending.opens.push(SourceOpenRequest::Path(path));
    }
}

#[on_command(OpenSourceView)]
pub(crate) fn on_open_source_view(
    trigger: On<OpenSourceView>,
    mut pending: ResMut<PendingSourceRequests>,
) {
    pending
        .opens
        .push(SourceOpenRequest::Asset(trigger.event().asset_path.clone()));
}

#[on_command(OpenTwinSource)]
pub(crate) fn on_open_twin_source(
    trigger: On<OpenTwinSource>,
    mut pending: ResMut<PendingSourceRequests>,
) {
    pending.opens.push(SourceOpenRequest::Twin {
        root: PathBuf::from(&trigger.event().twin_root),
        relative: PathBuf::from(&trigger.event().relative_path),
        pinned: trigger.event().pinned,
    });
}

#[on_command(SaveSourceText)]
pub(crate) fn on_save_source_text(
    trigger: On<SaveSourceText>,
    mut pending: ResMut<PendingSourceRequests>,
) {
    pending.saves.push(trigger.event().clone());
}

pub(crate) fn drain_pending_source_requests(world: &mut World) {
    let (opens, saves) = {
        let mut pending = world.resource_mut::<PendingSourceRequests>();
        (
            std::mem::take(&mut pending.opens),
            std::mem::take(&mut pending.saves),
        )
    };

    for request in opens {
        match request {
            SourceOpenRequest::Path(path) if !path.starts_with("mem://") => {
                open_path(world, PathBuf::from(path), None, None, false);
            }
            SourceOpenRequest::Path(_) => {}
            SourceOpenRequest::Asset(asset_path) => {
                let asset = {
                    let Some(manifest) =
                        world.get_resource::<lunco_assets::discovery::AssetManifest>()
                    else {
                        continue;
                    };
                    let Some(roots) = world.get_resource::<lunco_assets::twin_source::TwinRoots>()
                    else {
                        continue;
                    };
                    lunco_assets::discovery::list_all_assets(manifest, roots)
                        .into_iter()
                        .find(|asset| asset.asset_path == asset_path)
                };
                if let Some(asset) = asset {
                    let path = asset.abs_path.clone();
                    open_path(world, path, None, Some(asset), false);
                } else {
                    warn!("[SourceEditor] rejected unregistered asset path: {asset_path}");
                }
            }
            SourceOpenRequest::Twin {
                root,
                relative,
                pinned,
            } => {
                let Some((root, relative, path)) = resolve_twin_file(world, &root, &relative)
                else {
                    warn!(
                        "[SourceEditor] rejected path outside an open Twin: {}",
                        relative.display()
                    );
                    continue;
                };
                open_path(
                    world,
                    path,
                    Some(TwinSourceOrigin {
                        twin_root: root,
                        relative_path: relative,
                    }),
                    None,
                    pinned,
                );
            }
        }
    }

    for command in saves {
        start_write(world, command);
    }
}

fn open_path(
    world: &mut World,
    path: PathBuf,
    origin: Option<TwinSourceOrigin>,
    asset: Option<lunco_assets::discovery::AssetFile>,
    pinned: bool,
) {
    let existing = world
        .resource::<EditorTabs<SourceTabState>>()
        .find(|state| state.path == path);
    if let Some(tab) = existing {
        if pinned {
            world.resource_mut::<EditorTabs<SourceTabState>>().pin(tab);
        }
        world.trigger(OpenTab {
            kind: SOURCE_EDITOR_KIND,
            instance: tab,
        });
        return;
    }

    let state = || SourceTabState {
        path: path.clone(),
        text: String::new(),
        origin,
        dirty: false,
        loading: true,
        saving: false,
        error: None,
        request: 0,
    };
    let (tab, evicted) = if pinned {
        (
            world
                .resource_mut::<EditorTabs<SourceTabState>>()
                .ensure_pinned(|state| state.path == path, state),
            None,
        )
    } else {
        world
            .resource_mut::<EditorTabs<SourceTabState>>()
            .ensure_preview(|state| state.path == path, state)
    };
    if let Some(evicted) = evicted {
        world
            .resource_mut::<EditorTabs<SourceTabState>>()
            .close(evicted);
        world.trigger(CloseTab {
            kind: SOURCE_EDITOR_KIND,
            instance: evicted,
        });
    }
    start_read(world, tab, path, asset);
    world.trigger(OpenTab {
        kind: SOURCE_EDITOR_KIND,
        instance: tab,
    });
}

fn start_read(
    world: &mut World,
    tab: EditorTabId,
    path: PathBuf,
    asset: Option<lunco_assets::discovery::AssetFile>,
) {
    let request = {
        let mut reads = world.resource_mut::<PendingSourceReads>();
        reads.next_request = reads.next_request.wrapping_add(1);
        reads.next_request
    };
    if let Some(editor) = world
        .resource_mut::<EditorTabs<SourceTabState>>()
        .get_mut(tab)
    {
        editor.state.request = request;
    }

    #[cfg(not(target_arch = "wasm32"))]
    let task = AsyncComputeTaskPool::get().spawn(async move {
        if let Some(asset) = asset {
            lunco_assets::asset_read::read_asset_text(&asset).await
        } else {
            lunco_storage::read_file_sync(&path)
                .map_err(|error| format!("failed to read {}: {error:?}", path.display()))
                .and_then(|bytes| {
                    String::from_utf8(bytes)
                        .map_err(|error| format!("{}: not UTF-8 ({error})", path.display()))
                })
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    world
        .resource_mut::<PendingSourceReads>()
        .tasks
        .push(PendingSourceRead { tab, request, task });

    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = crossbeam_channel::bounded(1);
        wasm_bindgen_futures::spawn_local(async move {
            let read = if let Some(asset) = asset {
                lunco_assets::asset_read::read_asset_text(&asset).await
            } else {
                lunco_storage::read_file_sync(&path)
                    .map_err(|error| format!("failed to read {}: {error:?}", path.display()))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map_err(|error| format!("{}: not UTF-8 ({error})", path.display()))
                    })
            };
            let _ = tx.send(read);
        });
        world
            .resource_mut::<PendingSourceReads>()
            .tasks
            .push(PendingSourceRead {
                tab,
                request,
                result,
            });
    }
}

pub(crate) fn drain_pending_source_reads(world: &mut World) {
    let tasks = std::mem::take(&mut world.resource_mut::<PendingSourceReads>().tasks);
    let mut pending = Vec::new();
    for mut read in tasks {
        #[cfg(not(target_arch = "wasm32"))]
        let result = block_on(future::poll_once(&mut read.task));
        #[cfg(target_arch = "wasm32")]
        let result = read.result.try_recv().ok();
        let Some(result) = result else {
            pending.push(read);
            continue;
        };
        let mut tabs = world.resource_mut::<EditorTabs<SourceTabState>>();
        let Some(tab) = tabs.get_mut(read.tab) else {
            continue;
        };
        if tab.state.request != read.request {
            continue;
        }
        tab.state.loading = false;
        match result {
            Ok(text) => {
                tab.state.text = text;
                tab.state.error = None;
            }
            Err(error) => {
                warn!("[SourceEditor] {error}");
                tab.state.error = Some(error);
            }
        }
    }
    world.resource_mut::<PendingSourceReads>().tasks = pending;
}

fn start_write(world: &mut World, command: SaveSourceText) {
    let root = PathBuf::from(&command.twin_root);
    let relative = PathBuf::from(&command.relative_path);
    let Some((_, _, path)) = resolve_twin_file(world, &root, &relative) else {
        return;
    };
    let Some(tab) = world
        .resource::<EditorTabs<SourceTabState>>()
        .find(|state| state.path == path)
    else {
        return;
    };
    let bytes = command.text.clone().into_bytes();
    #[cfg(not(target_arch = "wasm32"))]
    let task = {
        let path = path.clone();
        AsyncComputeTaskPool::get().spawn(async move {
            lunco_storage::write_file_sync(&path, &bytes)
                .map_err(|error| format!("failed to save {}: {error:?}", path.display()))
        })
    };
    #[cfg(not(target_arch = "wasm32"))]
    world
        .resource_mut::<PendingSourceWrites>()
        .tasks
        .push(PendingSourceWrite {
            tab,
            path,
            saved_text: command.text,
            update: command.update,
            task,
        });
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = crossbeam_channel::bounded(1);
        let path_for_task = path.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let saved = lunco_storage::write_file_sync(&path_for_task, &bytes)
                .map_err(|error| format!("failed to save {}: {error:?}", path_for_task.display()));
            let _ = tx.send(saved);
        });
        world
            .resource_mut::<PendingSourceWrites>()
            .tasks
            .push(PendingSourceWrite {
                tab,
                path,
                saved_text: command.text,
                update: command.update,
                result,
            });
    }
}

pub(crate) fn drain_pending_source_writes(world: &mut World) {
    let tasks = std::mem::take(&mut world.resource_mut::<PendingSourceWrites>().tasks);
    let mut pending = Vec::new();
    for mut write in tasks {
        #[cfg(not(target_arch = "wasm32"))]
        let result = block_on(future::poll_once(&mut write.task));
        #[cfg(target_arch = "wasm32")]
        let result = write.result.try_recv().ok();
        let Some(result) = result else {
            pending.push(write);
            continue;
        };
        let succeeded = result.is_ok();
        if let Some(tab) = world
            .resource_mut::<EditorTabs<SourceTabState>>()
            .get_mut(write.tab)
        {
            tab.state.saving = false;
            match result {
                Ok(()) => {
                    if tab.state.text == write.saved_text {
                        tab.state.dirty = false;
                    }
                    tab.state.error = None;
                }
                Err(error) => tab.state.error = Some(error),
            }
        }
        if succeeded && write.update {
            world.trigger(OpenFile {
                path: write.path.to_string_lossy().into_owned(),
            });
        }
    }
    world.resource_mut::<PendingSourceWrites>().tasks = pending;
}

/// Claim close requests for source-editor instances and leave every other
/// editor family's request in the shared workbench queue.
pub(crate) fn drain_source_tab_closes(world: &mut World) {
    let requested = world.resource_mut::<PendingTabCloses>().drain();
    let mut unclaimed = Vec::new();
    for tab in requested {
        let TabId::Instance { kind, instance } = tab else {
            unclaimed.push(tab);
            continue;
        };
        if kind != SOURCE_EDITOR_KIND {
            unclaimed.push(tab);
            continue;
        }
        let dirty = world
            .resource::<EditorTabs<SourceTabState>>()
            .get(instance)
            .is_some_and(|tab| tab.state.dirty);
        if dirty {
            if let Some(tab) = world
                .resource_mut::<EditorTabs<SourceTabState>>()
                .get_mut(instance)
            {
                tab.state.error = Some("Save the file before closing this tab.".into());
            }
            continue;
        }
        world
            .resource_mut::<EditorTabs<SourceTabState>>()
            .close(instance);
        world.trigger(CloseTab { kind, instance });
    }
    let mut pending = world.resource_mut::<PendingTabCloses>();
    for tab in unclaimed {
        pending.push(tab);
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn resolve_twin_file(
    world: &World,
    requested_root: &Path,
    relative: &Path,
) -> Option<(PathBuf, PathBuf, PathBuf)> {
    if !safe_relative_path(relative) {
        return None;
    }
    let root = world
        .get_resource::<lunco_workspace::WorkspaceResource>()?
        .twins()
        .map(|(_, twin)| &twin.root)
        .find(|root| root.as_path() == requested_root)?
        .clone();
    Some((root.clone(), relative.to_path_buf(), root.join(relative)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twin_relative_paths_cannot_escape_the_root() {
        assert!(safe_relative_path(Path::new("models/rover.mo")));
        assert!(!safe_relative_path(Path::new("../outside.mo")));
        assert!(!safe_relative_path(Path::new("/absolute.mo")));
        assert!(!safe_relative_path(Path::new("")));
    }

    #[test]
    fn source_editor_is_a_center_instance_panel() {
        let panel = SourceEditorPanel;
        assert_eq!(panel.kind(), SOURCE_EDITOR_KIND);
        assert_eq!(panel.default_slot(), PanelSlot::Center);
    }
}
