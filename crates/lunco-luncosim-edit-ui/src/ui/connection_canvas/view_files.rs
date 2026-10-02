//! Typed view-document gestures and asynchronous project-file persistence.

use super::{UsdCanvasSessionState, UsdCanvasState};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_core::{ActiveCommandId, Command, CommandResults, on_command, register_commands};
use lunco_doc::diagram_view::{DiagramPosition, DiagramView, DiagramViewOp, DiagramViews};
use lunco_doc::{Ack, Document, DocumentId, Mutation, OpId};

/// Create an independent named scope and layout in the bound diagram view document.
#[Command(default)]
pub struct CreateConnectionView {
    pub view_id: u64,
    pub name: String,
    pub scope: String,
}
/// Place a composed source node in one named view without modifying USD.
#[Command(default)]
pub struct MoveConnectionViewNode {
    pub view_id: u64,
    pub view: String,
    pub path: String,
    pub x: f64,
    pub y: f64,
}
/// Undo or redo a view-document edit independently of source-document history.
#[Command(default)]
pub struct UndoConnectionView {
    pub view_id: u64,
    pub redo: bool,
}
/// Save or load named views asynchronously; damaged optional layouts recover with warnings.
#[Command(default)]
pub struct ConnectionViewFile {
    pub view_id: u64,
    pub path: String,
    pub save: bool,
    pub scope: String,
    pub include_descendants: bool,
}

fn state_for(views: &mut UsdCanvasState, id: u64) -> Result<&mut UsdCanvasSessionState, String> {
    std::iter::once(&mut views.scene)
        .chain(views.sessions.values_mut())
        .find(|state| {
            state
                .view_document
                .as_ref()
                .is_some_and(|host| host.document().id() == DocumentId::new(id))
        })
        .ok_or_else(|| "Diagram view document is closed or replaced".into())
}

#[on_command(CreateConnectionView)]
fn create_view(
    trigger: On<CreateConnectionView>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    let result = (|| {
        if !state.diagram_roots.contains(&cmd.scope) {
            return Err("Requested USD scope does not exist".into());
        }
        let host = state
            .view_document
            .as_mut()
            .ok_or("View document is unavailable")?;
        if host.document().data().views.contains_key(&cmd.name) {
            return Err("View name already exists".into());
        }
        let ack = host
            .apply(Mutation::local(DiagramViewOp::SetView {
                name: cmd.name.clone(),
                view: Some(DiagramView {
                    scope: cmd.scope.clone(),
                    include_descendants: false,
                    positions: Default::default(),
                }),
            }))
            .map_err(|error| error.to_string())?;
        state.include_descendants = false;
        state.selected_view = cmd.name.clone();
        state.diagram_root = cmd.scope.clone();
        state.rebuild_view();
        Ok(ack)
    })();
    report(state, &result);
    result
}

#[on_command(MoveConnectionViewNode)]
fn move_node(
    trigger: On<MoveConnectionViewNode>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    let result = (|| {
        if !state.source_prim_paths.contains(&cmd.path) {
            return Err("Moved node is absent from the composed USD source".into());
        }
        if cmd.x.abs() > f64::from(f32::MAX) || cmd.y.abs() > f64::from(f32::MAX) {
            return Err("Position exceeds the canvas rendering range".into());
        }
        state
            .view_document
            .as_mut()
            .ok_or("View document is unavailable")?
            .apply(Mutation::local(DiagramViewOp::SetPosition {
                view: cmd.view.clone(),
                path: cmd.path.clone(),
                position: Some(DiagramPosition { x: cmd.x, y: cmd.y }),
            }))
            .map_err(|error| error.to_string())
    })();
    report(state, &result);
    if result.is_err() {
        state.rebuild_view();
    } else {
        state.restore_placements();
    }
    result
}

#[on_command(UndoConnectionView)]
fn undo_view(
    trigger: On<UndoConnectionView>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    let result = (|| {
        let host = state
            .view_document
            .as_mut()
            .ok_or("View document is unavailable")?;
        if cmd.redo { host.redo() } else { host.undo() }.map_err(|error| error.to_string())?;
        if !host
            .document()
            .data()
            .views
            .contains_key(&state.selected_view)
        {
            state.selected_view = host
                .document()
                .data()
                .views
                .keys()
                .next()
                .ok_or("View document has no named view")?
                .clone();
        }
        state.include_descendants =
            host.document().data().views[&state.selected_view].include_descendants;
        state.diagram_root = host.document().data().views[&state.selected_view]
            .scope
            .clone();
        if !state.diagram_roots.contains(&state.diagram_root) {
            state.diagram_root = "/".into();
        }
        state.rebuild_view();
        Ok(Ack::new(OpId::new()))
    })();
    report(state, &result);
    result
}

fn report(state: &mut UsdCanvasSessionState, result: &Result<Ack, String>) {
    state.last_error = result.as_ref().err().cloned();
    if let Err(error) = result {
        warn!("[connection-view] rejected: {error}");
    }
}

/// One external persistence adapter: salvage usable TOML sections, then lower
/// to typed view facts. Corruption never changes the bound topology source.
fn read_views(text: &str, source: &str) -> (DiagramViews, Vec<String>) {
    let automatic = || {
        lunco_doc::diagram_view::DiagramViewDocument::new(source.into())
            .data()
            .clone()
    };
    let mut warnings = Vec::new();
    let table = match toml::from_str::<toml::Table>(text) {
        Ok(table) => table,
        Err(error) => {
            warnings.push(format!(
                "Damaged view file: {error}; recovered valid sections"
            ));
            let mut table = toml::Table::new();
            let mut section = String::new();
            let mut sections = Vec::new();
            for line in text.lines() {
                if line.trim_start().starts_with('[') && !section.trim().is_empty() {
                    sections.push(std::mem::take(&mut section));
                }
                section.push_str(line);
                section.push('\n');
            }
            if !section.trim().is_empty() {
                sections.push(section);
            }
            for section in sections {
                if let Ok(parsed) = toml::from_str::<toml::Table>(&section) {
                    merge_tables(&mut table, parsed);
                } else {
                    let mut header = "";
                    for line in section.lines() {
                        if line.trim_start().starts_with('[') {
                            header = line;
                            continue;
                        }
                        let candidate = if header.is_empty() {
                            line.to_string()
                        } else {
                            format!("{header}\n{line}")
                        };
                        if let Ok(parsed) = toml::from_str::<toml::Table>(&candidate) {
                            merge_tables(&mut table, parsed);
                        }
                    }
                }
            }
            table
        }
    };
    if table.get("version").and_then(toml::Value::as_integer) != Some(1) {
        warnings.push("Missing or unsupported view version; using automatic layout".into());
        return (automatic(), warnings);
    }
    if table.get("source").and_then(toml::Value::as_str) != Some(source) {
        warnings.push(
            "View source is missing or differs from the loaded USD; using automatic layout".into(),
        );
        return (automatic(), warnings);
    }
    let mut data = DiagramViews {
        version: 1,
        source: source.into(),
        views: Default::default(),
    };
    if let Some(views) = table.get("views").and_then(toml::Value::as_table) {
        for (name, value) in views {
            let Some(table) = value.as_table() else {
                warnings.push(format!("Skipped broken view {name}"));
                continue;
            };
            if name.trim().is_empty() {
                warnings.push("Skipped unnamed view".into());
                continue;
            }
            let scope = match table
                .get("scope")
                .and_then(toml::Value::as_str)
                .filter(|value| value.starts_with('/'))
            {
                Some(scope) => scope.to_string(),
                None => {
                    warnings.push(format!(
                        "View {name} has an invalid scope; showing the source root"
                    ));
                    "/".into()
                }
            };
            let mut view = DiagramView {
                scope,
                include_descendants: table
                    .get("include_descendants")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                positions: Default::default(),
            };
            if let Some(positions) = table.get("positions").and_then(toml::Value::as_table) {
                for (path, value) in positions {
                    let coordinate = |name| {
                        value
                            .get(name)
                            .and_then(|value| {
                                value.as_float().or_else(|| {
                                    value
                                        .as_integer()
                                        .filter(|v| v.unsigned_abs() <= (1_u64 << 53))
                                        .map(|v| v as f64)
                                })
                            })
                            .filter(|value| value.is_finite() && value.abs() <= f64::from(f32::MAX))
                    };
                    match (coordinate("x"), coordinate("y")) {
                        (Some(x), Some(y)) if path.starts_with('/') => {
                            view.positions
                                .insert(path.clone(), DiagramPosition { x, y });
                        }
                        _ => warnings.push(format!(
                            "Skipped broken placement {name}:{path}; automatic placement applies"
                        )),
                    }
                }
            } else if table.contains_key("positions") {
                warnings.push(format!(
                    "View {name} has invalid placements; using automatic layout"
                ));
            }
            data.views.insert(name.clone(), view);
        }
    }
    if data.views.is_empty() {
        warnings.push("No usable named views; using automatic layout".into());
        data = automatic();
    }
    (data, warnings)
}

fn merge_tables(target: &mut toml::Table, source: toml::Table) {
    for (key, value) in source {
        match (target.get_mut(&key), value) {
            (Some(toml::Value::Table(target)), toml::Value::Table(source)) => {
                merge_tables(target, source)
            }
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damaged_view_sections_keep_usable_layout_and_wrong_sources_use_automatic() {
        let text = "version = 1\nsource = 'scene'\n[views.Power]\nscope = '/Power'\n[views.Power.positions.'/A']\nx = 12.0\ny = 24.0\n[views.Power.positions.'/B']\nx = broken syntax\ny = 1.0\n[views.Other]\nscope = '/Other'\n";
        let (data, warnings) = read_views(text, "scene");
        assert!(!warnings.is_empty());
        assert_eq!(data.views["Power"].positions["/A"].x, 12.0);
        assert!(!data.views["Power"].positions.contains_key("/B"));
        assert_eq!(data.views["Other"].scope, "/Other");
        data.validate().unwrap();
        let (data, warnings) = read_views(text, "different");
        assert!(!warnings.is_empty());
        assert!(data.views["Overview"].positions.is_empty());
        let (data, warnings) = read_views("broken =", "scene");
        assert!(!warnings.is_empty());
        assert_eq!(data.source, "scene");
        let (data, warnings) = read_views(
            "version = 1\nsource = 'scene'\n[views.Main]\nscope = '/'\n[views.Main.positions.'/A']\nx = nan\ny = 1.0\n[views.Main.positions.'/B']\nx = 2.0\ny = 3.0",
            "scene",
        );
        assert!(!warnings.is_empty());
        assert!(!data.views["Main"].positions.contains_key("/A"));
        assert_eq!(data.views["Main"].positions["/B"].y, 3.0);
        let exported = toml::to_string_pretty(&data).unwrap();
        let (roundtrip, warnings) = read_views(&exported, "scene");
        assert!(warnings.is_empty());
        assert_eq!(data, roundtrip);
    }
}

struct FileResult {
    data: Option<DiagramViews>,
    path: String,
    generation: u64,
    result: Result<(), String>,
    warnings: Vec<String>,
    command_id: Option<u64>,
}
#[derive(Resource, Default)]
pub struct PendingViewFiles {
    tasks: std::collections::HashMap<u64, Task<FileResult>>,
}

#[on_command(ConnectionViewFile)]
fn view_file(
    trigger: On<ConnectionViewFile>,
    mut views: ResMut<UsdCanvasState>,
    mut pending: ResMut<PendingViewFiles>,
    command: Res<ActiveCommandId>,
    mut results: ResMut<CommandResults>,
) {
    let cmd = trigger.event();
    let queued = (|| -> Result<(), String> {
        if cmd.path.trim().is_empty() || !cmd.path.ends_with(".lunco-view.toml") {
            return Err("Choose a repository file ending in .lunco-view.toml".into());
        }
        if pending.tasks.contains_key(&cmd.view_id) {
            return Err("A view file operation is already in progress".into());
        }
        if std::iter::once(&views.scene)
            .chain(views.sessions.values())
            .any(|state| {
                (!state.view_file_path.is_empty()
                    && lunco_doc::same_file(
                        std::path::Path::new(&state.view_file_path),
                        std::path::Path::new(&cmd.path),
                    ))
                    && state
                        .view_document
                        .as_ref()
                        .is_some_and(|host| host.document().id().raw() != cmd.view_id)
            })
        {
            return Err("This view file is already bound to another diagram document".into());
        }
        let state = state_for(&mut views, cmd.view_id)?;
        let host = state
            .view_document
            .as_mut()
            .ok_or("View document is unavailable")?;
        if cmd.save {
            let mut definition = host
                .document()
                .data()
                .views
                .get(&state.selected_view)
                .ok_or("Selected view does not exist")?
                .clone();
            definition.scope = cmd.scope.clone();
            definition.include_descendants = cmd.include_descendants;
            if definition != host.document().data().views[&state.selected_view] {
                host.apply(Mutation::local(DiagramViewOp::SetView {
                    name: state.selected_view.clone(),
                    view: Some(definition),
                }))
                .map_err(|error| error.to_string())?;
            }
        }
        let data = host.document().data().clone();
        let generation = host.document().generation();
        let path = cmd.path.clone();
        let save = cmd.save;
        let command_id = command.get();
        pending.tasks.insert(
            cmd.view_id,
            AsyncComputeTaskPool::get().spawn(async move {
                let mut loaded = None;
                let mut warnings = Vec::new();
                let result = (|| -> Result<(), String> {
                    let file = std::path::Path::new(&path);
                    if save {
                        data.validate().map_err(|error| error.to_string())?;
                        let bytes =
                            toml::to_string_pretty(&data).map_err(|error| error.to_string())?;
                        lunco_storage::write_file_sync(file, bytes.as_bytes())
                            .map_err(|error| error.to_string())?;
                    } else {
                        match lunco_storage::read_text_file_sync(file) {
                            Ok(text) => {
                                let (imported, issues) = read_views(&text, &data.source);
                                warnings = issues;
                                loaded = Some(imported);
                            }
                            Err(error) => {
                                warnings.push(format!(
                                    "View file unavailable: {error}; using automatic layout"
                                ));
                                loaded = Some(
                                    lunco_doc::diagram_view::DiagramViewDocument::new(
                                        data.source.clone(),
                                    )
                                    .data()
                                    .clone(),
                                );
                            }
                        }
                    }
                    Ok(())
                })();
                FileResult {
                    data: loaded,
                    path,
                    generation,
                    result,
                    warnings,
                    command_id,
                }
            }),
        );
        state.last_error = None;
        if let Some(id) = command.get() {
            results.insert(id, lunco_core::CommandOutcome::Pending);
        }
        Ok(())
    })();
    if let Err(error) = queued {
        warn!("[connection-view] file command rejected: {error}");
        if let Ok(state) = state_for(&mut views, cmd.view_id) {
            state.last_error = Some(error.clone());
        }
        if let Some(id) = command.get() {
            results.record(id, Err(error));
        }
    }
}

pub fn view_files_pending(pending: Res<PendingViewFiles>) -> bool {
    !pending.tasks.is_empty()
}
pub fn poll_view_files(
    mut pending: ResMut<PendingViewFiles>,
    mut views: ResMut<UsdCanvasState>,
    mut results: ResMut<CommandResults>,
) {
    let completed: Vec<_> = pending
        .tasks
        .iter_mut()
        .filter_map(|(id, task)| {
            future::block_on(future::poll_once(task)).map(|result| (*id, result))
        })
        .collect();
    for (id, mut completed) in completed {
        pending.tasks.remove(&id);
        let result = (|| -> Result<Ack, String> {
            completed.result?;
            let warnings = &mut completed.warnings;
            let state = state_for(&mut views, id)?;
            let host = state
                .view_document
                .as_mut()
                .ok_or("View document is unavailable")?;
            if let Some(data) = completed.data {
                if host.document().generation() != completed.generation {
                    return Err(
                        "Layout changed while the view file was loading; load was not applied"
                            .into(),
                    );
                }
                host.apply(Mutation::local(DiagramViewOp::Replace(data)))
                    .map_err(|error| error.to_string())?;
                state.selected_view = host
                    .document()
                    .data()
                    .views
                    .keys()
                    .next()
                    .ok_or("View file has no named views")?
                    .clone();
                state.include_descendants =
                    host.document().data().views[&state.selected_view].include_descendants;
                state.diagram_root = host.document().data().views[&state.selected_view]
                    .scope
                    .clone();
                if !state.diagram_roots.contains(&state.diagram_root) {
                    warnings.push(format!(
                        "USD scope {} is unavailable; showing the source root",
                        state.diagram_root
                    ));
                    state.diagram_root = "/".into();
                }
                state.saved_view_generation = if warnings.is_empty() {
                    host.document().generation()
                } else {
                    completed.generation
                };
                state.rebuild_view();
            } else {
                state.saved_view_generation = completed.generation;
            }
            state.view_file_path = completed.path;
            Ok(Ack::new(OpId::new()))
        })();
        if let Ok(state) = state_for(&mut views, id) {
            report(state, &result);
            if result.is_ok() && !completed.warnings.is_empty() {
                let warning = completed.warnings.join("; ");
                warn!("[connection-view] {warning}");
                state.last_error = Some(warning);
            }
        } else if let Err(error) = &result {
            warn!("[connection-view] {error}");
        }
        if let Some(command_id) = completed.command_id {
            results.record(command_id, result);
        }
    }
}

register_commands!(create_view, move_node, undo_view, view_file);
pub fn init_view_commands(app: &mut App) {
    register_all_commands(app);
    app.init_resource::<PendingViewFiles>();
    app.init_resource::<lunco_api::queries::ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
        .register(InspectConnectionDiagram);
}

struct InspectConnectionDiagram;
impl lunco_api::queries::ApiQueryProvider for InspectConnectionDiagram {
    fn name(&self) -> &'static str {
        "InspectConnectionDiagram"
    }
    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        lunco_api_core::ApiQuerySchema { name: self.name().into(), description: Some("Inspect the active Connections source, named view definitions, and rendered node/port coordinates".into()), parameters: Some(Vec::new()), exactly_one_of: Vec::new(), response: Some("{ built, active_scene, source, view_id, view, scope, definitions:[{name,scope,include_descendants,positions:[{path,x,y}]}], nodes:[{path,label,x,y,screen_x,screen_y,ports:[{name,kind,x,y,screen_x,screen_y}]}], connection_count, unresolved_links, error }".into()) }
    }
    fn execute(
        &self,
        world: &World,
        _params: &lunco_api_core::ApiValue,
    ) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let Some(views) = world.get_resource::<UsdCanvasState>() else {
            return Err(lunco_api::ApiQueryError::new(
                lunco_api_core::ApiErrorCode::InternalError,
                "Connections producer is unavailable",
            ));
        };
        let state = if views.show_scene {
            Some(&views.scene)
        } else {
            world
                .get_resource::<lunco_usd_viewport_core::UsdViewportState>()
                .and_then(|viewport| viewport.focused_preview_id())
                .and_then(|preview| views.sessions.get(&preview))
        };
        let Some(state) = state else {
            return Ok(Some(api_value!({ "built": false })));
        };
        let nodes: Vec<_> = state.canvas.scene.nodes().map(|(_, node)| {
            let ports: Vec<_> = node.ports.iter().map(|port| {
                let pos = port.world_pos(node.rect);
                let screen = state.canvas_rect.map(|rect| state.canvas.viewport.world_to_screen(pos, rect));
                api_value!({ "name": port.id.as_str(), "kind": port.kind.as_str(), "x": pos.x, "y": pos.y, "screen_x": screen.map(|p| p.x), "screen_y": screen.map(|p| p.y) })
            }).collect();
            let screen = state.canvas_rect.map(|rect| state.canvas.viewport.world_to_screen(node.rect.center(), rect));
            api_value!({ "path": node.origin.clone(), "label": node.label.clone(), "x": node.rect.min.x, "y": node.rect.min.y, "screen_x": screen.map(|p| p.x), "screen_y": screen.map(|p| p.y), "ports": ports })
        }).collect();
        let definitions: Vec<_> = state.view_document.as_ref().map(|host| host.document().data().views.iter().map(|(name, definition)| {
            let positions: Vec<_> = definition.positions.iter().map(|(path, pos)| api_value!({ "path": path, "x": pos.x, "y": pos.y })).collect();
            api_value!({ "name": name, "scope": definition.scope.clone(), "include_descendants": definition.include_descendants, "positions": positions })
        }).collect()).unwrap_or_default();
        Ok(Some(
            api_value!({ "built": state.built, "active_scene": views.show_scene, "source": state.source_uri.clone(), "view_id": state.view_document.as_ref().map(|host| host.document().id().raw()), "view": state.selected_view.clone(), "scope": state.diagram_root.clone(), "definitions": definitions, "nodes": nodes, "connection_count": state.canvas.scene.edge_count(), "unresolved_links": state.unresolved_links.clone(), "error": state.last_error.clone() }),
        ))
    }
}
