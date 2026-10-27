//! Scene-scoped demonstration HUD over the native signal grid and plot controls.

use bevy::{ecs::system::SystemState, prelude::*};
use bevy_egui::EguiContexts;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, GlobalEntityId, SceneResource, on_command, register_commands};
use lunco_workbench_core::{InstancePanel, MenuCtx, PanelCtx, presentation::HelpAnchors};
use std::collections::BTreeMap;

use super::signal_grid::{
    ConfigureSignalGrid, SignalGridPanel, SignalGrids, configure_signal_grid,
};
use crate::kinds::line_plot::{
    ConfigureLinePlot, LinePlotStyle, render_appearance_menu, toolbar_signal_catalog,
};
use crate::{SignalExposure, SignalGridEntry, SignalRef, SignalRegistry, VisualizationRegistry};

const MAX_PARAMETERS: usize = 4;
const MAX_HUDS: usize = 8;
const PRIMARY_HUD: u64 = 1;

/// Choose measured parameters and control their native viewport graphs.
/// Omitted fields retain state; an empty signal list clears the selection.
#[Command(default)]
pub struct SetGraphHud {
    /// Omission selects the primary HUD, creating it on first use.
    pub hud: Option<u64>,
    pub visible: Option<bool>,
    pub signals: Option<Vec<SignalGridEntry>>,
    /// One or two columns, at most the selected parameter count.
    pub columns: Option<u32>,
    /// Card dimensions in logical pixels, including its frame.
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Optional paired logical-pixel position inside the application window.
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// The HUD owns its grid through scene teardown; signal history stays shared.
#[derive(Clone)]
struct GraphHud {
    visible: bool,
    grid: Option<u64>,
    size: egui::Vec2,
    position: Option<egui::Pos2>,
    apply_size: bool,
    apply_position: bool,
}

impl Default for GraphHud {
    fn default() -> Self {
        Self {
            visible: false,
            grid: None,
            size: egui::vec2(640.0, 360.0),
            position: None,
            apply_size: true,
            apply_position: false,
        }
    }
}

/// Collection and allocation belong to the current scene, including hidden HUDs.
#[derive(Resource)]
pub(crate) struct GraphHuds {
    instances: BTreeMap<u64, GraphHud>,
    next_id: u64,
}

impl Default for GraphHuds {
    fn default() -> Self {
        Self {
            instances: BTreeMap::new(),
            next_id: PRIMARY_HUD,
        }
    }
}

impl SceneResource for GraphHuds {}

/// Add an independent, initially empty telemetry HUD. Returns its instance id.
#[Command(default)]
pub struct CreateGraphHud {}

/// Close one HUD and release only its native grid and plots.
#[Command(default)]
pub struct RemoveGraphHud {
    pub hud: u64,
}

#[on_command(CreateGraphHud)]
fn on_create_graph_hud(_: On<CreateGraphHud>, mut huds: ResMut<GraphHuds>) -> Result<Ack, String> {
    if huds.instances.len() >= MAX_HUDS {
        return Err(format!("at most {MAX_HUDS} graph HUDs may be open"));
    }
    let id = huds.next_id;
    huds.next_id = id.checked_add(1).ok_or("HUD identity space exhausted")?;
    huds.instances.insert(
        id,
        GraphHud {
            visible: true,
            ..Default::default()
        },
    );
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({"hud": id}),
    ))
}

#[on_command(RemoveGraphHud)]
fn on_remove_graph_hud(
    trigger: On<RemoveGraphHud>,
    mut huds: ResMut<GraphHuds>,
    mut grids: ResMut<SignalGrids>,
    mut plots: ResMut<VisualizationRegistry>,
) -> Result<Ack, String> {
    let hud = huds
        .instances
        .remove(&trigger.hud)
        .ok_or("graph HUD is unavailable")?;
    if let Some(grid) = hud.grid {
        grids.remove(grid, &mut plots);
    }
    Ok(Ack::new(OpId::new()))
}

#[on_command(SetGraphHud)]
fn on_set_graph_hud(
    trigger: On<SetGraphHud>,
    entities: Res<lunco_api::registry::ApiEntityRegistry>,
    signals: Res<SignalRegistry>,
    mut plots: ResMut<VisualizationRegistry>,
    mut grids: ResMut<SignalGrids>,
    mut huds: ResMut<GraphHuds>,
    theme: Res<lunco_theme::Theme>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let id = request.hud.unwrap_or(PRIMARY_HUD);
    let mut hud = match huds.instances.get(&id) {
        Some(hud) => hud.clone(),
        None if request.hud.is_none() && huds.instances.len() < MAX_HUDS => GraphHud::default(),
        None => return Err("graph HUD is unavailable; create it before selecting its id".into()),
    };
    let known_colours: Vec<_> = huds
        .instances
        .values()
        .filter_map(|hud| hud.grid)
        .filter_map(|grid| grids.plots(grid))
        .flatten()
        .filter_map(|id| plots.get(*id))
        .filter_map(|config| {
            Some((
                config.inputs.first()?.source.clone(),
                LinePlotStyle::load(config).series_color?,
            ))
        })
        .collect();
    configure_hud(
        request,
        &mut hud,
        &entities,
        &signals,
        &mut plots,
        &mut grids,
        &theme,
        &known_colours,
    )?;
    huds.instances.insert(id, hud);
    huds.next_id = huds.next_id.max(id + 1);
    Ok(Ack::new(OpId::new()))
}

fn configure_hud(
    request: &SetGraphHud,
    hud: &mut GraphHud,
    entities: &lunco_api::registry::ApiEntityRegistry,
    signals: &SignalRegistry,
    plots: &mut VisualizationRegistry,
    grids: &mut SignalGrids,
    theme: &lunco_theme::Theme,
    known_colours: &[(SignalRef, [u8; 3])],
) -> Result<(), String> {
    let position = match (request.x, request.y) {
        (None, None) => None,
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(egui::pos2(x, y)),
        _ => return Err("HUD position requires finite x and y coordinates together".into()),
    };
    let size = egui::vec2(
        request.width.unwrap_or(hud.size.x),
        request.height.unwrap_or(hud.size.y),
    );
    if !size.is_finite() || size.x < 320.0 || size.y < 240.0 {
        return Err(
            "graph HUD dimensions must be finite and at least 320 × 240 logical pixels".into(),
        );
    }
    if let Some(entries) = &request.signals {
        if entries.len() > MAX_PARAMETERS {
            return Err(format!(
                "graph HUD supports at most {MAX_PARAMETERS} parameters"
            ));
        }
        if entries.is_empty() {
            if request.columns.is_some() {
                return Err("choose parameters before setting columns".into());
            }
            if let Some(grid) = hud.grid.take() {
                grids.remove(grid, plots);
            }
        } else {
            let palette = theme
                .plot
                .series
                .iter()
                .map(|c| [c.r(), c.g(), c.b()])
                .fold(Vec::new(), |mut colours, colour| {
                    if !colours.contains(&colour) {
                        colours.push(colour);
                    }
                    colours
                });
            if palette.len() < entries.len() {
                return Err(
                    "the active plot palette needs a distinct colour per HUD parameter".into(),
                );
            }
            let columns = request.columns.unwrap_or(entries.len().min(2) as u32);
            if !(1..=2).contains(&columns) || columns as usize > entries.len() {
                return Err(
                    "graph HUD requires one or two columns, at most the parameter count".into(),
                );
            }
            let grid = hud.grid.unwrap_or_else(|| grids.allocate_id());
            let previous = grids.plots(grid).unwrap_or(&[]).to_vec();
            let window = previous
                .first()
                .and_then(|id| plots.get(*id))
                .map(|config| LinePlotStyle::load(config).time_window_s)
                .unwrap_or(Some(15.0));
            configure_signal_grid(
                &ConfigureSignalGrid {
                    grid,
                    title: "Telemetry".into(),
                    columns,
                    signals: entries.clone(),
                    sync_time_axes: Some(true),
                },
                entities,
                signals,
                plots,
                grids,
            )?;
            let mut used: Vec<_> = grids
                .plots(grid)
                .unwrap_or(&[])
                .iter()
                .filter(|id| previous.contains(id))
                .filter_map(|id| plots.get(*id))
                .filter_map(|config| LinePlotStyle::load(config).series_color)
                .collect();
            for id in grids.plots(grid).unwrap_or(&[]) {
                if previous.contains(id) {
                    continue;
                }
                let config = plots.get_mut(*id).expect("configured grid owns its plots");
                let mut style = LinePlotStyle::load(config);
                // Native presentation defaults; each plot retains its ordinary
                // appearance controls and ConfigureLinePlot API afterwards.
                style.show_controls = false;
                style.show_legend = false;
                style.show_hover_values = false;
                style.show_latest_value = true;
                style.compact_layout = true;
                style.simplified_grid = true;
                style.follow_latest = true;
                style.time_window_s = window;
                let colour = known_colours
                    .iter()
                    .find(|(source, colour)| {
                        source == &config.inputs[0].source && !used.contains(colour)
                    })
                    .map(|(_, colour)| *colour)
                    .or_else(|| {
                        palette.iter().copied().find(|colour| {
                            !used.contains(colour)
                                && !known_colours.iter().any(|(_, known)| known == colour)
                        })
                    })
                    .or_else(|| {
                        palette
                            .iter()
                            .copied()
                            .find(|colour| !used.contains(colour))
                    })
                    .expect("validated palette has a free HUD colour");
                used.push(colour);
                style.series_color = Some(colour);
                style.line_width = 3.5;
                style.text_size = Some(16.0);
                style.x_label = Some("Time (s)".into());
                style.save(config);
            }
            hud.grid = Some(grid);
        }
    } else if request.columns.is_some() {
        return Err("include the selected signals when changing the column count".into());
    }
    if let Some(visible) = request.visible {
        hud.visible = visible;
    }
    if request.width.is_some() || request.height.is_some() {
        hud.apply_size = true;
    }
    hud.size = size;
    if let Some(position) = position {
        hud.position = Some(position);
        hud.apply_position = true;
    }
    Ok(())
}

/// Retire hidden HUD plots at the same boundary as their scene-owned selection.
pub(crate) fn retire_graph_hud(
    huds: Res<GraphHuds>,
    mut grids: ResMut<SignalGrids>,
    mut plots: ResMut<VisualizationRegistry>,
) {
    for hud in huds.instances.values() {
        if let Some(grid) = hud.grid {
            grids.remove(grid, &mut plots);
        }
    }
}

/// Native window geometry is the presentation owner's measured layout fact.
#[derive(Event)]
pub(crate) struct GraphHudMeasured {
    id: u64,
    rect: egui::Rect,
}

pub(crate) fn on_graph_hud_measured(trigger: On<GraphHudMeasured>, mut huds: ResMut<GraphHuds>) {
    let Some(hud) = huds.instances.get_mut(&trigger.id) else {
        return;
    };
    if !trigger.rect.is_finite() {
        return;
    }
    hud.size = trigger.rect.size();
    hud.position = Some(trigger.rect.min);
    hud.apply_size = false;
    hud.apply_position = false;
}

/// Construct the ordinary native grid entry from a live channel's identity.
pub(crate) fn signal_entry(
    ctx: &PanelCtx<'_>,
    source: &SignalRef,
) -> Result<SignalGridEntry, String> {
    let signals = ctx.resource_expect::<SignalRegistry>();
    let target_gid = signals
        .global_owner(source)
        .or_else(|| ctx.get::<GlobalEntityId>(source.entity).copied())
        .ok_or_else(|| "This parameter has no stable scene owner.".to_string())?;
    let meta = signals.meta(source);
    Ok(SignalGridEntry {
        target_gid,
        signal: source.path.clone(),
        caption: crate::display_channel_label(
            &source.path,
            meta.and_then(|meta| meta.group_path.as_deref()),
            meta.and_then(|meta| meta.unit.as_deref()),
            false,
        ),
    })
}

fn selected_entries(ctx: &PanelCtx<'_>, hud: &GraphHud) -> Result<Vec<SignalGridEntry>, String> {
    let Some(grid) = hud.grid else {
        return Ok(Vec::new());
    };
    let grids = ctx.resource_expect::<SignalGrids>();
    let ids = grids
        .plots(grid)
        .ok_or("The selected graph grid is unavailable.")?;
    let plots = ctx.resource_expect::<VisualizationRegistry>();
    ids.iter()
        .map(|id| {
            let config = plots.get(*id).ok_or("A selected graph is unavailable.")?;
            let binding = config
                .inputs
                .first()
                .ok_or("A selected graph has no parameter.")?;
            let mut entry = signal_entry(ctx, &binding.source)?;
            entry.caption = config.title.clone();
            Ok(entry)
        })
        .collect()
}

/// Shared Settings → HUD entry. Parameter selection lives on the viewport card.
pub fn graph_hud_menu_ui(ui: &mut egui::Ui, ctx: &mut MenuCtx<'_>) {
    let Some(huds) = ctx.resource::<GraphHuds>() else {
        return;
    };
    let rows: Vec<_> = huds
        .instances
        .iter()
        .map(|(id, hud)| (*id, hud.visible))
        .collect();
    ui.label(egui::RichText::new("Telemetry graphs").strong());
    if rows.is_empty() {
        let mut enabled = false;
        if ui.checkbox(&mut enabled, "Show graph HUD").changed() {
            ctx.trigger(SetGraphHud {
                visible: Some(enabled),
                ..Default::default()
            });
        }
    }
    for (id, mut visible) in rows {
        if ui
            .checkbox(&mut visible, format!("Telemetry {id}"))
            .changed()
        {
            ctx.trigger(SetGraphHud {
                hud: Some(id),
                visible: Some(visible),
                ..Default::default()
            });
        }
    }
    if ui.button("+ New graph HUD").clicked() {
        ctx.trigger(CreateGraphHud {});
    }
}

/// Browse the shared native catalog by source and composed component path.
fn parameter_picker(
    ui: &mut egui::Ui,
    ctx: &PanelCtx<'_>,
    hud_id: u64,
    selected: &[SignalGridEntry],
) -> Option<Vec<SignalGridEntry>> {
    use lunco_workbench_widgets::tree;
    let key = egui::Id::new(("graph_hud_parameter_picker", hud_id));
    let (mut search, mut internal) = crate::scene_state::scene_data(ui.ctx(), |data| {
        data.get_temp::<(String, bool)>(key).unwrap_or_default()
    });
    ui.set_width(400.0_f32.min(ui.ctx().content_rect().width() - 32.0));
    ui.label(egui::RichText::new("Parameters").strong());
    ui.label(
        egui::RichText::new(format!("{} of {MAX_PARAMETERS} selected", selected.len()))
            .weak()
            .small(),
    );
    let mut changed = None;
    if !selected.is_empty() {
        for (index, entry) in selected.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&entry.caption);
                if ui
                    .small_button("×")
                    .on_hover_text("Remove this parameter")
                    .clicked()
                {
                    let mut next = selected.to_vec();
                    next.remove(index);
                    changed = Some(next);
                }
            });
        }
        ui.separator();
    }
    ui.add(
        egui::TextEdit::singleline(&mut search)
            .hint_text("Filter names, components or descriptions…")
            .desired_width(f32::INFINITY),
    );
    ui.checkbox(&mut internal, "Include internal variables");
    let signals = ctx.resource_expect::<SignalRegistry>();
    let catalog = toolbar_signal_catalog(ui.ctx(), signals);
    let search_lower = search.to_lowercase();
    let mut groups = BTreeMap::<(String, u64), BTreeMap<String, Vec<_>>>::new();
    for item in &catalog.signals {
        if !signals.is_active(&item.source) {
            continue;
        }
        if !internal
            && item.meta.as_ref().is_some_and(|meta| {
                meta.exposure == SignalExposure::Internal
                    || meta
                        .canonical_name
                        .as_ref()
                        .is_some_and(|name| name != &item.source.path)
            })
        {
            continue;
        }
        let owner = lunco_core::entity_display_name(
            ctx.get::<Name>(item.source.entity),
            ctx.get::<lunco_core::markers::Callsign>(item.source.entity),
            ctx.get::<lunco_core::CatalogEntryId>(item.source.entity),
        );
        let owner = if owner.is_empty() {
            "Global signals".into()
        } else {
            owner
        };
        let component = item
            .meta
            .as_ref()
            .and_then(|meta| meta.group_path.clone())
            .unwrap_or_default();
        if !search_lower.is_empty()
            && !item.source.path.to_lowercase().contains(&search_lower)
            && !item
                .hint
                .as_ref()
                .is_some_and(|hint| hint.to_lowercase().contains(&search_lower))
            && !component.to_lowercase().contains(&search_lower)
            && !owner.to_lowercase().contains(&search_lower)
        {
            continue;
        }
        groups
            .entry((owner, item.source.entity.to_bits()))
            .or_default()
            .entry(component)
            .or_default()
            .push(item);
    }
    let contains_selected = |items: &[&crate::kinds::line_plot::ToolbarSignal]| {
        items.iter().any(|item| {
            signal_entry(ctx, &item.source).is_ok_and(|entry| {
                selected
                    .iter()
                    .any(|old| old.target_gid == entry.target_gid && old.signal == entry.signal)
            })
        })
    };
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for ((owner, entity_bits), components) in &groups {
                let count = components.values().map(Vec::len).sum::<usize>();
                let owner_selected = components.values().any(|items| contains_selected(items));
                tree::branch(
                    ui,
                    key.with(("owner", entity_bits)),
                    owner_selected,
                    (!search_lower.is_empty()).then_some(true),
                    |ui| {
                        tree::label(
                            ui,
                            format!("{} ({count})", crate::humanize_identifier(owner)),
                            ui.available_width(),
                            egui::Sense::click(),
                        )
                        .clicked()
                    },
                    |ui| {
                        for (path, items) in components {
                            let component = path
                                .rsplit('/')
                                .find(|part| !part.is_empty())
                                .map(crate::humanize_identifier)
                                .unwrap_or_else(|| "Signals".into());
                            tree::branch(
                                ui,
                                key.with(("component", entity_bits, path)),
                                contains_selected(items),
                                (!search_lower.is_empty()).then_some(true),
                                |ui| {
                                    tree::label(
                                        ui,
                                        format!("{component} ({})", items.len()),
                                        ui.available_width(),
                                        egui::Sense::click(),
                                    )
                                    .on_hover_text(path)
                                    .clicked()
                                },
                                |ui| {
                                    for item in items {
                                        let entry = signal_entry(ctx, &item.source);
                                        let existing = entry.as_ref().ok().and_then(|entry| {
                                            selected.iter().position(|old| {
                                                old.target_gid == entry.target_gid
                                                    && old.signal == entry.signal
                                            })
                                        });
                                        let mut checked = existing.is_some();
                                        let label = entry
                                            .as_ref()
                                            .map(|entry| entry.caption.as_str())
                                            .unwrap_or(&item.source.path);
                                        let unit = item
                                            .meta
                                            .as_ref()
                                            .and_then(|meta| meta.unit.as_deref())
                                            .filter(|unit| !unit.is_empty())
                                            .unwrap_or("—");
                                        let ready = signals
                                            .scalar_history(&item.source)
                                            .is_some_and(|history| !history.is_empty());
                                        let enabled = entry.is_ok()
                                            && ready
                                            && (checked || selected.len() < MAX_PARAMETERS);
                                        let text = format!("{label}  [{unit}]");
                                        tree::leaf(ui, |ui| {
                                            let response = ui
                                                .add_enabled(
                                                    enabled,
                                                    egui::Checkbox::new(&mut checked, text),
                                                )
                                                .on_hover_text(
                                                    item.hint
                                                        .as_deref()
                                                        .unwrap_or(&item.source.path),
                                                );
                                            if response.clicked() {
                                                let mut next = selected.to_vec();
                                                if let Some(index) = existing {
                                                    next.remove(index);
                                                } else if let Ok(entry) = entry {
                                                    next.push(entry);
                                                }
                                                changed = Some(next);
                                            }
                                        });
                                    }
                                },
                            );
                        }
                    },
                );
            }
            if groups.is_empty() {
                ui.label("No active parameters match this filter.");
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!selected.is_empty(), egui::Button::new("Clear selection"))
            .clicked()
        {
            changed = Some(Vec::new());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Done").clicked() {
                ui.close();
            }
        });
    });
    crate::scene_state::scene_data(ui.ctx(), |data| data.insert_temp(key, (search, internal)));
    changed
}

fn controls(ui: &mut egui::Ui, ctx: &mut PanelCtx<'_>, hud_id: u64, hud: &GraphHud) {
    let entries = selected_entries(ctx, hud);
    let plots: Vec<_> = hud
        .grid
        .and_then(|grid| ctx.resource_expect::<SignalGrids>().plots(grid))
        .unwrap_or(&[])
        .iter()
        .filter_map(|id| {
            ctx.resource_expect::<VisualizationRegistry>()
                .get_shared(*id)
        })
        .collect();
    let mut requested = None;
    ui.horizontal(|ui| {
        egui::containers::menu::MenuButton::new(format!("Parameters ({})", plots.len()))
            .config(
                egui::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            )
            .ui(ui, |ui| match &entries {
                Ok(entries) => requested = parameter_picker(ui, ctx, hud_id, entries),
                Err(error) => {
                    ui.label(error);
                    if ui.button("Clear selection").clicked() {
                        requested = Some(Vec::new());
                    }
                }
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("×")
                .on_hover_text("Close this HUD")
                .clicked()
            {
                ctx.trigger(RemoveGraphHud { hud: hud_id });
            }
            if ui
                .small_button("+")
                .on_hover_text("New independent graph HUD")
                .clicked()
            {
                ctx.trigger(CreateGraphHud {});
            }
            ui.menu_button("Style", |ui| {
                ui.label("Drag the edges or corner to resize this HUD.");
                ui.label("Card size");
                for (label, width, height) in [
                    ("Compact", 420.0, 280.0),
                    ("Standard", 640.0, 360.0),
                    ("Wide", 900.0, 360.0),
                ] {
                    if ui.button(label).clicked() {
                        ctx.trigger(SetGraphHud {
                            hud: Some(hud_id),
                            width: Some(width),
                            height: Some(height),
                            ..Default::default()
                        });
                    }
                }
                if let Ok(entries) = &entries {
                    if entries.len() > 1 {
                        let columns = hud
                            .grid
                            .and_then(|grid| ctx.resource_expect::<SignalGrids>().columns(grid));
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Columns");
                            for count in 1..=2 {
                                if ui
                                    .selectable_label(columns == Some(count), count.to_string())
                                    .clicked()
                                {
                                    ctx.trigger(SetGraphHud {
                                        hud: Some(hud_id),
                                        signals: Some(entries.clone()),
                                        columns: Some(count),
                                        ..Default::default()
                                    });
                                }
                            }
                        });
                    }
                }
                ui.separator();
                for config in &plots {
                    ui.menu_button(&config.title, |ui| {
                        let style = LinePlotStyle::load_cached(ui.ctx(), config);
                        let span = style.time_window_s.or_else(|| {
                            let signals = ctx.resource_expect::<SignalRegistry>();
                            config
                                .inputs
                                .first()
                                .and_then(|binding| signals.scalar_history(&binding.source))
                                .and_then(|history| {
                                    Some(history.back()?.time - history.front()?.time)
                                })
                                .filter(|span| *span > 0.0)
                        });
                        if let Some(span) = span {
                            render_appearance_menu(ui, ctx, config.id.raw(), &style, span);
                        } else {
                            ui.label("Waiting for measured history.");
                        }
                    });
                }
            });
            if !plots.is_empty() {
                let window = LinePlotStyle::load_cached(ui.ctx(), &plots[0]).time_window_s;
                let uniform = plots.iter().all(|config| {
                    LinePlotStyle::load_cached(ui.ctx(), config).time_window_s == window
                });
                let label = if !uniform {
                    "Mixed history".into()
                } else {
                    window.map_or_else(|| "All history".into(), |seconds| format!("{seconds:.0} s"))
                };
                ui.menu_button(label, |ui| {
                    ui.label("History window");
                    for seconds in [5.0, 15.0, 30.0, 60.0, 0.0] {
                        let active = uniform && window == (seconds > 0.0).then_some(seconds);
                        let label = if seconds == 0.0 {
                            "All retained data".into()
                        } else {
                            format!("{seconds:.0} seconds")
                        };
                        if ui.selectable_label(active, label).clicked() {
                            for config in &plots {
                                ctx.trigger(ConfigureLinePlot {
                                    plot: config.id.raw(),
                                    time_window_s: Some(seconds),
                                    follow_latest: Some(true),
                                    ..Default::default()
                                });
                            }
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Choose the history window for all parameters");
            }
        });
    });
    if let Some(entries) = requested {
        ctx.trigger(SetGraphHud {
            hud: Some(hud_id),
            signals: Some(entries),
            ..Default::default()
        });
    }
}

/// Paint independent native windows over the shared measured scene area.
pub fn draw_graph_hud(world: &mut World, state: &mut SystemState<EguiContexts>) {
    let huds: Vec<_> = world
        .resource::<GraphHuds>()
        .instances
        .iter()
        .filter(|(_, hud)| hud.visible)
        .map(|(id, hud)| (*id, hud.clone()))
        .collect();
    if huds.is_empty() {
        return;
    }
    let ctx = {
        let Ok(mut contexts) = state.get_mut(world) else {
            return;
        };
        let Ok(ctx) = contexts.ctx_mut() else { return };
        ctx.clone()
    };
    let Some(viewport) = world
        .get_resource::<HelpAnchors>()
        .and_then(|anchors| anchors.get("panel.center"))
    else {
        return;
    };
    let theme = world.resource::<lunco_theme::Theme>();
    let (padding, fill, border, rounding) = (
        theme.spacing.window_padding,
        theme.tokens.overlay_backdrop,
        theme.tokens.overlay_border,
        theme.rounding.window,
    );
    let bounds = viewport.shrink(padding);
    if bounds.width() < 320.0 || bounds.height() < 240.0 {
        return;
    }
    // One tier parent keeps native focus ordering between its window children.
    let hud_layer =
        lunco_theme::HudTier::Specific.layer(&ctx, egui::Id::new("lunco_graph_hud_windows"));
    let mut intents = Vec::new();
    for (index, (id, hud)) in huds.iter().enumerate() {
        let size = hud.size.min(bounds.size());
        let window_id = egui::Id::new(("lunco_graph_hud", id));
        let mut window = egui::Window::new(format!("Telemetry {id}"))
            .id(window_id)
            .title_bar(false)
            .collapsible(false)
            .order(lunco_theme::HudTier::ORDER)
            .constrain_to(bounds)
            .default_pos(
                bounds.right_bottom() - size - egui::Vec2::splat(index as f32 * padding * 3.0),
            )
            .default_size(size)
            .min_size([320.0, 240.0])
            .max_size(bounds.size())
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0, border))
                    .corner_radius(rounding)
                    .inner_margin(egui::Margin::same(padding as i8)),
            );
        // A requested size is applied once through the native resize owner;
        // subsequent frames keep the operator's native drag-resize interaction.
        if hud.apply_size {
            window = window.fixed_size(size);
        }
        if hud.apply_position {
            if let Some(position) = hud.position {
                window = window.current_pos(position);
            }
        }
        let mut panel = PanelCtx::new(world);
        let response = window.show(&ctx, |ui| {
            ui.set_min_size(ui.available_size());
            controls(ui, &mut panel, *id, hud);
            ui.add_space(padding);
            if let Some(grid) = hud.grid {
                SignalGridPanel.render(ui, &mut panel, grid);
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(padding * 3.0);
                    ui.heading("Your simulation, in graphs");
                    ui.label("Choose up to four measured parameters above.");
                    ui.label(
                        egui::RichText::new(
                            "Browse sources and components. Drag this window to position it.",
                        )
                        .weak(),
                    );
                });
            }
        });
        intents.push(panel.take_intents());
        if let Some(response) = response {
            ctx.set_sublayer(hud_layer, response.response.layer_id);
            world.trigger(GraphHudMeasured {
                id: *id,
                rect: response.response.rect,
            });
            if let Some(mut anchors) = world.get_resource_mut::<HelpAnchors>() {
                anchors.set(format!("hud.graph.{id}"), response.response.rect);
            }
            if let Some(mut gate) =
                world.get_resource_mut::<lunco_workbench_core::scene_pick::ScenePickGate>()
            {
                gate.record_chrome_panel(response.response.rect, response.response.rect);
            }
        }
    }
    // Measurements precede user intents so a size preset requested this frame
    // remains pending for the next native-window pass.
    for intent in intents {
        intent.apply(world);
    }
}

pub(crate) struct InspectGraphHudProvider;

impl lunco_api::queries::ApiQueryProvider for InspectGraphHudProvider {
    fn name(&self) -> &'static str {
        "InspectGraphHud"
    }
    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        lunco_api_core::ApiQuerySchema {
            name: self.name().into(),
            description: Some(
                "Inspect native HUD parameters, appearance and measured rendering bounds.".into(),
            ),
            parameters: Some(vec![]),
            exactly_one_of: vec![],
            response: None,
        }
    }
    fn execute(&self, world: &World, _: &lunco_api_core::ApiValue) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let huds = world.resource::<GraphHuds>();
        let anchors = world.get_resource::<HelpAnchors>();
        let viewport = anchors.and_then(|anchors| anchors.get("panel.center"));
        let grids = world.resource::<SignalGrids>();
        let plots = world.resource::<VisualizationRegistry>();
        let signals = world.resource::<SignalRegistry>();
        let reports: Vec<_> = huds.instances.iter().map(|(id, hud)| {
            let rect = anchors.and_then(|anchors| anchors.get(&format!("hud.graph.{id}")));
            let ids = hud.grid.and_then(|grid| grids.plots(grid)).unwrap_or(&[]);
            let entries: Vec<_> = ids.iter().filter_map(|id| plots.get(*id)).filter_map(|config| {
                let binding = config.inputs.first()?;
                let style = LinePlotStyle::load(config);
                Some(api_value!({"plot": config.id.raw(), "signal": binding.source.path.clone(),
                    "target_gid": signals.global_owner(&binding.source).or_else(|| world.get::<GlobalEntityId>(binding.source.entity).copied()).map(|id| id.get()),
                    "caption": config.title.clone(), "unit": signals.meta(&binding.source).and_then(|meta| meta.unit.clone()),
                    "active": signals.is_active(&binding.source), "samples": signals.scalar_history(&binding.source).map(|history| history.len()),
                    "line_width": style.line_width, "text_size": style.text_size, "time_window_s": style.time_window_s,
                    "series_color": style.series_color, "show_controls": style.show_controls, "compact_layout": style.compact_layout,
                }))
            }).collect();
            api_value!({"hud": *id, "visible": hud.visible, "grid": hud.grid,
                "plots": ids.iter().map(|id| id.raw()).collect::<Vec<_>>(), "signals": entries,
                "columns": hud.grid.and_then(|grid| grids.columns(grid)), "width": hud.size.x, "height": hud.size.y,
                "position": hud.position.map(|position| [position.x, position.y]), "rendered": hud.visible && rect.is_some(),
                "rect": rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]),
                "viewport": viewport.map(|r| [r.min.x, r.min.y, r.width(), r.height()]),
            })
        }).collect();
        Ok(Some(api_value!({"huds": reports})))
    }
}

register_commands!(on_set_graph_hud, on_create_graph_hud, on_remove_graph_hud);
