//! Quick-open and document navigation in the concrete shell.

use super::*;
use lunco_workbench_core::commands::{ActivateNavigationEntry, NavigateHistory, SearchNavigation};
#[cfg(feature = "api")]
use lunco_workbench_core::navigation::NavigationSource;
use lunco_workbench_core::navigation::{NavigationEntry, NavigationRegistry};
use lunco_workbench_core::trigger_or_defer;

#[derive(Resource, Default)]
struct NavigationSearch {
    open: bool,
    query: String,
    category: String,
    entries: Vec<NavigationEntry>,
    selected: usize,
    error: Option<String>,
    focus: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Location {
    perspective: PerspectiveId,
    tab: Option<TabId>,
}

#[derive(Resource)]
struct NavigationHistory {
    locations: Vec<Location>,
    cursor: usize,
    capacity: usize,
}

impl Default for NavigationHistory {
    fn default() -> Self {
        Self {
            locations: Vec::new(),
            cursor: 0,
            capacity: 128,
        }
    }
}

enum Request {
    Search(SearchNavigation),
    Activate(String),
    History(bool),
}

#[derive(Resource, Default)]
struct Requests(Vec<(Request, Option<u64>, Option<lunco_workspace::TwinId>)>);

fn scope(workspace: Option<&WorkspaceResource>) -> Option<lunco_workspace::TwinId> {
    workspace.and_then(|workspace| workspace.active_twin)
}

#[on_command(SearchNavigation)]
fn on_search(
    trigger: On<SearchNavigation>,
    mut requests: ResMut<Requests>,
    active: Res<ActiveCommandId>,
    workspace: Option<Res<WorkspaceResource>>,
) -> CommandOutcome {
    requests.0.push((
        Request::Search(trigger.event().clone()),
        active.get(),
        scope(workspace.as_deref()),
    ));
    CommandOutcome::Pending
}

#[on_command(ActivateNavigationEntry)]
fn on_activate(
    trigger: On<ActivateNavigationEntry>,
    mut requests: ResMut<Requests>,
    active: Res<ActiveCommandId>,
    workspace: Option<Res<WorkspaceResource>>,
) -> CommandOutcome {
    requests.0.push((
        Request::Activate(trigger.id.clone()),
        active.get(),
        scope(workspace.as_deref()),
    ));
    CommandOutcome::Pending
}

#[on_command(NavigateHistory)]
fn on_history(
    trigger: On<NavigateHistory>,
    mut requests: ResMut<Requests>,
    active: Res<ActiveCommandId>,
    workspace: Option<Res<WorkspaceResource>>,
) -> CommandOutcome {
    requests.0.push((
        Request::History(trigger.forward),
        active.get(),
        scope(workspace.as_deref()),
    ));
    CommandOutcome::Pending
}

lunco_core::register_commands!(on_search, on_activate, on_history);

fn reject(message: &str) -> Reject {
    Reject::InvalidOp(message.to_owned())
}

fn process_requests(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<Requests>().0);
    for (request, command, owner) in requests {
        let result = if owner != scope(world.get_resource::<WorkspaceResource>()) {
            Err(reject("Navigation request belongs to a retired Twin"))
        } else {
            apply_request(world, request)
        };
        if let Err(error) = &result {
            warn!("Navigation rejected: {error:?}");
            let message = format!("Navigation rejected: {error:?}");
            world.resource_mut::<NavigationSearch>().error = Some(message.clone());
            world.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "navigation-rejected".into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Warning,
                data: lunco_telemetry_core::TelemetryValue::String(message),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }
        if let Some(command) = command {
            world
                .resource_mut::<CommandResults>()
                .record_handler_result(command, result);
        }
    }
}

fn apply_request(world: &mut World, request: Request) -> Result<Ack, Reject> {
    match request {
        Request::Search(request) => {
            let active_twin = scope(world.get_resource::<WorkspaceResource>()).is_some();
            let initial_scope = if active_twin { "Twin" } else { "Actions" };
            let category = if request.query.trim().is_empty() && request.category.is_empty() {
                initial_scope
            } else {
                request.category.as_str()
            };
            let inventory = world
                .resource::<NavigationRegistry>()
                .entries(world, category);
            let mut entries = inventory.entries;
            entries.retain(|entry| {
                entry.matches(&request.query)
                    && (request.category.is_empty() || entry.category == request.category)
                    && (!request.query.trim().is_empty()
                        || !request.category.is_empty()
                        || entry.category == if active_twin { "Twin" } else { "Actions" })
            });
            let rank = |entry: &NavigationEntry| match entry.category.as_str() {
                "Twin" => 0,
                "Scenes" => 1,
                "Libraries" => 2,
                _ => 3,
            };
            entries.sort_by(|a, b| {
                rank(a)
                    .cmp(&rank(b))
                    .then(a.label.cmp(&b.label))
                    .then(a.detail.cmp(&b.detail))
            });
            let mut search = world.resource_mut::<NavigationSearch>();
            let opening = !search.open;
            search.open = true;
            search.query = request.query;
            search.category = request.category;
            search.entries = entries;
            search.selected = 0;
            search.error = if inventory.errors.is_empty() {
                None
            } else {
                Some(inventory.errors.join("; "))
            };
            search.focus = opening;
        }
        Request::Activate(id) => {
            // Resolve against the current owner inventory; a stored display row cannot authorize an action.
            let inventory = world.resource::<NavigationRegistry>().entries(world, "");
            let entry = inventory
                .entries
                .into_iter()
                .find(|entry| entry.id == id)
                .ok_or_else(|| {
                    reject(&format!(
                        "Navigation destination is no longer available{}",
                        if inventory.errors.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", inventory.errors.join("; "))
                        }
                    ))
                })?;
            let mut ctx = MenuCtx::new(world);
            (entry.activate)(&mut ctx);
            let intents = ctx.take_intents();
            world.resource_mut::<NavigationSearch>().open = false;
            world.resource_mut::<NavigationSearch>().error = None;
            intents.apply(world);
        }
        Request::History(forward) => {
            let history = world.resource::<NavigationHistory>();
            let cursor = if forward {
                history.cursor.checked_add(1)
            } else {
                history.cursor.checked_sub(1)
            }
            .filter(|index| *index < history.locations.len())
            .ok_or_else(|| reject("No navigation history in that direction"))?;
            let location = history.locations[cursor].clone();
            let mut layout = world.resource_mut::<WorkbenchLayout>();
            if !layout.activate_perspective_by_str(location.perspective.as_str()) {
                return Err(reject("The destination perspective is no longer available"));
            }
            if let Some(tab) = location.tab {
                if !layout.focus_existing_tab(tab) {
                    return Err(reject("The destination tab has been closed"));
                }
            }
            world.resource_mut::<NavigationHistory>().cursor = cursor;
            world.resource_mut::<NavigationSearch>().error = None;
        }
    }
    Ok(Ack::new(OpId::new()))
}

fn record_location(snapshot: Res<WorkbenchSnapshot>, mut history: ResMut<NavigationHistory>) {
    let Some(perspective) = snapshot.active_perspective() else {
        return;
    };
    let tab = snapshot
        .focused_tab()
        .filter(|tab| matches!(tab, TabId::Instance { .. }));
    if tab.is_none()
        && history
            .locations
            .get(history.cursor)
            .is_some_and(|location| location.perspective == perspective)
    {
        return;
    }
    let location = Location { perspective, tab };
    if history.locations.get(history.cursor) == Some(&location) {
        return;
    }
    let keep = if history.locations.is_empty() {
        0
    } else {
        history.cursor + 1
    };
    history.locations.truncate(keep);
    history.locations.push(location);
    if history.locations.len() > history.capacity {
        history.locations.remove(0);
    }
    history.cursor = history.locations.len() - 1;
}

fn retire(
    trigger: On<lunco_workspace::TwinClosed>,
    mut search: ResMut<NavigationSearch>,
    mut history: ResMut<NavigationHistory>,
) {
    if trigger.was_active {
        *search = NavigationSearch::default();
        *history = NavigationHistory::default();
    }
}

fn shell_entries(world: &World) -> Result<Vec<NavigationEntry>, String> {
    let layout = world
        .get_resource::<WorkbenchLayout>()
        .ok_or("Workbench layout unavailable")?;
    let mut entries = Vec::new();
    for (id, panel) in &layout.panels {
        if panel.menu_group() == PanelMenuGroup::Hidden {
            continue;
        }
        let target = id.as_str().to_owned();
        entries.push(NavigationEntry {
            id: format!("panel:{target}"),
            label: panel.dynamic_title(world),
            detail: "Open panel".into(),
            category: "Actions".into(),
            activate: Arc::new(move |ctx| ctx.trigger(FocusPanel { id: target.clone() })),
        });
    }
    for perspective in &layout.perspectives {
        let id = perspective.id().as_str().to_owned();
        entries.push(NavigationEntry {
            id: format!("perspective:{id}"),
            label: perspective.title(),
            detail: perspective.description().to_owned(),
            category: "Actions".into(),
            activate: Arc::new(move |ctx| {
                ctx.trigger(lunco_workbench_core::commands::ActivatePerspective { id: id.clone() })
            }),
        });
    }
    Ok(entries)
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<NavigationRegistry>()
        .init_resource::<NavigationSearch>()
        .init_resource::<NavigationHistory>()
        .init_resource::<Requests>();
    app.world_mut()
        .resource_mut::<NavigationRegistry>()
        .register(&["Actions"], shell_entries);
    register_all_commands(app);
    app.add_observer(retire)
        .add_systems(
            Update,
            process_requests.before(crate::drain_pending_layout_requests),
        )
        .add_systems(
            Update,
            record_location
                .after(WorkbenchSnapshotPublishSet)
                .run_if(resource_changed::<WorkbenchSnapshot>),
        );
    #[cfg(feature = "api")]
    {
        app.init_resource::<lunco_api::ApiQueryRegistry>();
        app.world_mut()
            .resource_mut::<lunco_api::ApiQueryRegistry>()
            .register(InspectNavigation);
    }
}

#[cfg(feature = "api")]
fn focused_source(layout: &WorkbenchLayout, world: &World) -> Option<NavigationSource> {
    let TabId::Instance { kind, instance } = *layout.focused_tab()? else {
        return None;
    };
    layout
        .instance_panels
        .get(&kind)?
        .navigation_source(world, instance)
}

pub(super) fn render_menu(ui: &mut egui::Ui, world: &mut World) {
    if crate::render::menu_item(
        ui,
        true,
        "Find files…",
        &crate::render::configured_command_shortcut(world, "navigation.find"),
        "",
    )
    .clicked()
    {
        trigger_or_defer(world, SearchNavigation::default());
        ui.close();
    }
    let history = world.resource::<NavigationHistory>();
    let back = history.cursor > 0;
    let forward = history.cursor + 1 < history.locations.len();
    for (label, enabled, forward, binding) in [
        ("Back", back, false, "navigation.back"),
        ("Forward", forward, true, "navigation.forward"),
    ] {
        if crate::render::menu_item(
            ui,
            enabled,
            label,
            &crate::render::configured_command_shortcut(world, binding),
            "No destination in that direction",
        )
        .clicked()
        {
            trigger_or_defer(world, NavigateHistory { forward });
            ui.close();
        }
    }
}

pub(super) fn render_search(ctx: &egui::Context, world: &mut World) {
    #[cfg(feature = "api")]
    if world
        .get_resource::<lunco_capture::screenshot::OfflineRecordingState>()
        .is_some_and(|recording| recording.active)
        && !world
            .get_resource::<OfflineRecordingPresentation>()
            .is_some_and(|presentation| presentation.retain_workbench_chrome)
    {
        return;
    }
    let mut search = world.resource_mut::<NavigationSearch>();
    if !search.open {
        return;
    }
    let mut open = true;
    let mut changed = None;
    let mut activate = None;
    let mut close = false;
    egui::Window::new("Go to…")
        .id(egui::Id::new("workbench_quick_open"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(620.0)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
        .show(ctx, |ui| {
            let mut query = search.query.clone();
            let field = ui.add(
                text_editor::singleline(&mut query)
                    .hint_text("Find files, scenes, libraries, or actions…")
                    .desired_width(f32::INFINITY),
            );
            if search.focus {
                field.request_focus();
                search.focus = false;
            }
            if field.changed() {
                changed = Some((query, search.category.clone()));
            }
            ui.horizontal(|ui| {
                for category in ["", "Twin", "Libraries", "Scenes", "Actions"] {
                    if ui
                        .selectable_label(
                            search.category == category,
                            if category.is_empty() { "All" } else { category },
                        )
                        .clicked()
                    {
                        changed = Some((search.query.clone(), category.to_owned()));
                    }
                }
            });
            if let Some(error) = &search.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if search.entries.is_empty() {
                ui.label("No matching destinations.");
            }
            let mut moved = false;
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
            {
                search.selected = (search.selected + 1).min(search.entries.len().saturating_sub(1));
                moved = true;
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                search.selected = search.selected.saturating_sub(1);
                moved = true;
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                activate = search
                    .entries
                    .get(search.selected)
                    .map(|entry| entry.id.clone());
            }
            close =
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            let title_height = ui.text_style_height(&egui::TextStyle::Body);
            let detail_height = ui.text_style_height(&egui::TextStyle::Small);
            let padding = ui.spacing().button_padding;
            let row_height = title_height + detail_height + padding.y * 2.0;
            let mut scroll = egui::ScrollArea::vertical().max_height(390.0);
            if moved {
                scroll = scroll.vertical_scroll_offset(
                    search.selected as f32 * (row_height + ui.spacing().item_spacing.y),
                );
            }
            scroll.show_rows(ui, row_height, search.entries.len(), |ui, range| {
                for index in range {
                    let entry = &search.entries[index];
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::click(),
                    );
                    if search.selected == index || response.hovered() {
                        let fill = if search.selected == index {
                            ui.visuals().selection.bg_fill
                        } else {
                            ui.visuals().widgets.hovered.bg_fill
                        };
                        ui.painter().rect_filled(
                            rect,
                            ui.visuals().widgets.inactive.corner_radius,
                            fill,
                        );
                    }
                    let mut row = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(padding))
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    row.spacing_mut().item_spacing.y = 0.0;
                    row.add(
                        egui::Label::new(format!("{}  ·  {}", entry.label, entry.category))
                            .truncate(),
                    );
                    row.add(
                        egui::Label::new(egui::RichText::new(&entry.detail).small()).truncate(),
                    );
                    let response = response.on_hover_text(&entry.detail);
                    if response.clicked() {
                        activate = Some(entry.id.clone());
                    }
                }
            });
        });
    search.open = open && !close;
    drop(search);
    if let Some((query, category)) = changed {
        trigger_or_defer(world, SearchNavigation { query, category });
    }
    if let Some(id) = activate {
        trigger_or_defer(world, ActivateNavigationEntry { id });
    }
}

#[cfg(feature = "api")]
struct InspectNavigation;

#[cfg(feature = "api")]
impl lunco_api::ApiQueryProvider for InspectNavigation {
    fn name(&self) -> &'static str {
        "InspectWorkbenchNavigation"
    }
    fn execute(&self, world: &World, _: &lunco_api_core::ApiValue) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let search = world.resource::<NavigationSearch>();
        let history = world.resource::<NavigationHistory>();
        let source = world
            .get_resource::<WorkbenchLayout>()
            .and_then(|layout| focused_source(layout, world));
        let workspace = world.get_resource::<WorkspaceResource>();
        let twin =
            workspace.and_then(|workspace| workspace.active_twin.and_then(|id| workspace.twin(id)));
        let scene = world
            .get_resource::<CurrentScenePath>()
            .map(|scene| scene.0.clone())
            .unwrap_or_default();
        let entries: Vec<_> = search.entries.iter().map(|entry| api_value!({ "id": entry.id.clone(), "label": entry.label.clone(), "detail": entry.detail.clone(), "category": entry.category.clone() })).collect();
        Ok(Some(
            api_value!({ "open": search.open, "query": search.query.clone(), "category": search.category.clone(), "entries": entries,
                "can_back": history.cursor > 0, "can_forward": history.cursor + 1 < history.locations.len(),
                "source": source.as_ref().map(|source| source.uri.clone()), "mode": source.map(|source| source.mode),
                "twin_id": scope(workspace).map(|id| id.raw()), "twin_name": twin.and_then(|twin| twin.manifest.as_ref().map(|manifest| manifest.name.clone())),
                "twin_root": twin.map(|twin| twin.root.display().to_string()), "scene_path": scene, "error": search.error.clone(),
            }),
        ))
    }
}
