//! Small multiples over the existing native plot and signal owners.
//! The grid owns composition, never a second history or measurement pipeline.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, GlobalEntityId, on_command, register_commands};
use lunco_workbench_core::{InstancePanel, PanelCtx, PanelId, PanelSlot};
use serde::{Deserialize, Serialize};

use crate::kinds::line_plot::{LINE_PLOT_KIND, LinePlotStyle};
use crate::panel::VizPanel;
use crate::view::ViewTarget;
use crate::viz::{SignalBinding, VisualizationConfig};
use crate::{SignalRef, SignalRegistry, VisualizationRegistry, VizId};

pub(crate) const SIGNAL_GRID_KIND: PanelId = PanelId("signal_grid");

#[derive(Clone, Debug, Reflect, Serialize, Deserialize)]
pub struct SignalGridEntry {
    pub target_gid: GlobalEntityId,
    pub signal: String,
    /// Human quantity name. Empty uses the shared signal label policy.
    pub caption: String,
}

/// Compose 1–64 measured scalar channels as independent native line plots.
/// Prepare first, inspect the returned plot identities with InspectSignalGrid,
/// then use ConfigureLinePlot for each plot's existing appearance settings.
#[Command(default)]
pub struct ConfigureSignalGrid {
    pub grid: u64,
    pub title: String,
    pub columns: u32,
    pub signals: Vec<SignalGridEntry>,
    /// Link the time axes and follow one common history interval. Defaults to
    /// true for a new grid; false leaves independently framed plots.
    pub sync_time_axes: Option<bool>,
}

#[derive(Clone)]
struct SignalGrid {
    title: String,
    columns: usize,
    plots: Vec<VizId>,
    sync_time_axes: bool,
}

// Like VisualizationRegistry, composition survives scene replacement. Its
// plot bindings reconnect through the existing persisted-source lifecycle.
#[derive(Resource, Default)]
pub(crate) struct SignalGrids(HashMap<u64, SignalGrid>);

impl SignalGrids {
    pub(super) fn allocate_id(&self) -> u64 {
        loop {
            let id = VizId::next().raw();
            if !self.0.contains_key(&id) {
                return id;
            }
        }
    }

    pub(super) fn plots(&self, id: u64) -> Option<&[VizId]> {
        self.0.get(&id).map(|grid| grid.plots.as_slice())
    }

    pub(super) fn columns(&self, id: u64) -> Option<u32> {
        self.0.get(&id).map(|grid| grid.columns as u32)
    }

    pub(super) fn remove(&mut self, id: u64, plots: &mut VisualizationRegistry) {
        if let Some(grid) = self.0.remove(&id) {
            for plot in grid.plots {
                plots.remove(plot);
            }
        }
    }
}

fn validate_shape(columns: u32, count: usize) -> Result<(), String> {
    if !(1..=64).contains(&count) || !(1..=8).contains(&columns) || columns as usize > count {
        return Err(
            "signal grid requires 1–64 channels and 1–8 columns, at most the channel count".into(),
        );
    }
    Ok(())
}

#[on_command(ConfigureSignalGrid)]
fn on_configure_signal_grid(
    trigger: On<ConfigureSignalGrid>,
    entities: Res<lunco_api::registry::ApiEntityRegistry>,
    signals: Res<SignalRegistry>,
    mut registry: ResMut<VisualizationRegistry>,
    mut grids: ResMut<SignalGrids>,
) -> Result<Ack, String> {
    configure_signal_grid(
        trigger.event(),
        &entities,
        &signals,
        &mut registry,
        &mut grids,
    )
}

/// Shared native grid mutation; all consumers validate before changing plots.
pub(super) fn configure_signal_grid(
    request: &ConfigureSignalGrid,
    entities: &lunco_api::registry::ApiEntityRegistry,
    signals: &SignalRegistry,
    registry: &mut VisualizationRegistry,
    grids: &mut SignalGrids,
) -> Result<Ack, String> {
    validate_shape(request.columns, request.signals.len())?;
    if request.grid == 0 {
        return Err("signal grid identity must be nonzero".into());
    }
    let mut seen = HashSet::new();
    // Validate every owner and descriptor before mutating the existing grid.
    let resolved = request
        .signals
        .iter()
        .map(|entry| {
            let entity = entities
                .resolve(&entry.target_gid)
                .ok_or_else(|| format!("signal owner {} is unavailable", entry.target_gid.get()))?;
            let source = SignalRef::new(entity, entry.signal.clone());
            if !seen.insert(source.clone()) {
                return Err("signal grid repeats a channel".into());
            }
            if !signals.is_active(&source) {
                return Err(format!("`{}` has no active publisher", entry.signal));
            }
            let history = signals.scalar_history(&source).ok_or_else(|| {
                format!(
                    "measured scalar `{}` is unavailable on {}",
                    entry.signal,
                    entry.target_gid.get()
                )
            })?;
            if history.is_empty() {
                return Err(format!("`{}` has no measured samples", entry.signal));
            }
            let meta = signals.meta(&source);
            let caption = if entry.caption.trim().is_empty() {
                crate::humanize_identifier(&entry.signal)
            } else {
                entry.caption.clone()
            };
            let unit = meta.and_then(|m| m.unit.clone());
            Ok((source, caption, unit))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let previous = grids
        .0
        .get(&request.grid)
        .map(|g| g.plots.clone())
        .unwrap_or_default();
    let mut plots = Vec::with_capacity(resolved.len());
    for (source, caption, unit) in resolved {
        let existing = previous.iter().copied().find(|id| {
            registry
                .get(*id)
                .is_some_and(|cfg| cfg.inputs.len() == 1 && cfg.inputs[0].source == source)
        });
        let id = existing.unwrap_or_else(|| registry.allocate_id());
        let mut config = existing
            .and_then(|id| registry.get(id).cloned())
            .unwrap_or_else(|| VisualizationConfig {
                id,
                title: caption.clone(),
                kind: LINE_PLOT_KIND,
                view: ViewTarget::Panel2D,
                inputs: vec![SignalBinding::live(source, "y")],
                style: serde_json::Value::Null,
            });
        config.title = caption.clone();
        let mut style = LinePlotStyle::load(&config);
        style.caption = Some(caption);
        if existing.is_none() {
            style.y_label = unit;
        }
        style.save(&mut config);
        registry.insert(config);
        plots.push(id);
    }
    for id in previous {
        if !plots.contains(&id) {
            registry.remove(id);
        }
    }
    let sync_time_axes = request.sync_time_axes.unwrap_or_else(|| {
        grids
            .0
            .get(&request.grid)
            .is_none_or(|grid| grid.sync_time_axes)
    });
    grids.0.insert(
        request.grid,
        SignalGrid {
            title: request.title.clone(),
            columns: request.columns as usize,
            plots,
            sync_time_axes,
        },
    );
    Ok(Ack::new(OpId::new()))
}

fn cell_rect(
    bounds: egui::Rect,
    columns: usize,
    count: usize,
    index: usize,
    gap: f32,
) -> egui::Rect {
    let rows = count.div_ceil(columns);
    let size = (bounds.size() - egui::vec2((columns - 1) as f32, (rows - 1) as f32) * gap)
        / egui::vec2(columns as f32, rows as f32);
    let offset = egui::vec2((index % columns) as f32, (index / columns) as f32)
        * (size + egui::Vec2::splat(gap));
    egui::Rect::from_min_size(bounds.min + offset, size)
}

pub(crate) struct SignalGridPanel;

impl InstancePanel for SignalGridPanel {
    fn kind(&self) -> PanelId {
        SIGNAL_GRID_KIND
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }
    fn title(&self, world: &World, instance: u64) -> String {
        world
            .get_resource::<SignalGrids>()
            .and_then(|g| g.0.get(&instance))
            .map(|g| g.title.clone())
            .unwrap_or_else(|| "Signal grid".into())
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        let Some(grid) = ctx
            .resource::<SignalGrids>()
            .and_then(|g| g.0.get(&instance))
            .cloned()
        else {
            ui.label("Prepare the signal grid before opening this view.");
            return;
        };
        let bounds = ui.available_rect_before_wrap();
        let time_axis = if grid.sync_time_axes {
            ctx.resource::<VisualizationRegistry>()
                .zip(ctx.resource::<SignalRegistry>())
                .and_then(|(plots, signals)| grid_time_span(&grid, plots, signals, Some(ui.ctx())))
                .map(|span_s| crate::view::SharedTimeAxis {
                    group: ui.id().with(("signal_grid_time", instance)),
                    span_s,
                })
        } else {
            None
        };
        let rows = grid.plots.len().div_ceil(grid.columns);
        let gap = 8.0_f32
            .min(bounds.width() / (grid.columns as f32 * 4.0))
            .min(bounds.height() / (rows as f32 * 4.0));
        for (index, plot) in grid.plots.iter().enumerate() {
            let rect = cell_rect(bounds, grid.columns, grid.plots.len(), index, gap);
            if rect.width() < 8.0 || rect.height() < 8.0 {
                continue;
            }
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .id_salt((instance, index)),
            );
            // The ordinary plot owns history caching, transforms, labels,
            // follow-latest and typed appearance controls in every cell.
            VizPanel.render_with_time_axis(&mut child, ctx, plot.raw(), time_axis);
        }
        ui.allocate_rect(bounds, egui::Sense::hover());
    }
}

/// Read endpoint metadata from the existing history owner; do not scan samples
/// or create another history/cache for the grid.
fn grid_time_span(
    grid: &SignalGrid,
    plots: &VisualizationRegistry,
    signals: &SignalRegistry,
    context: Option<&egui::Context>,
) -> Option<[f64; 2]> {
    crate::shared_time_axis::shared_time_span(
        grid.plots
            .iter()
            .filter_map(|id| plots.get(*id))
            .filter(|plot| {
                context
                    .map_or_else(
                        || LinePlotStyle::load(plot),
                        |context| LinePlotStyle::load_cached(context, plot),
                    )
                    .x_signal
                    .is_none()
            })
            .flat_map(|plot| plot.inputs.iter())
            .filter(|binding| binding.visible && binding.role == "y")
            .filter_map(|binding| signals.scalar_history(&binding.source))
            .filter_map(|history| Some([history.front()?.time, history.back()?.time])),
    )
}

pub(crate) struct InspectSignalGridProvider;

impl lunco_api::queries::ApiQueryProvider for InspectSignalGridProvider {
    fn name(&self) -> &'static str {
        "InspectSignalGrid"
    }
    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        lunco_api_core::ApiQuerySchema {
            name: self.name().into(),
            description: Some(
                "Inspect prepared native signal grids and their plot identities.".into(),
            ),
            parameters: Some(vec![]),
            exactly_one_of: vec![],
            response: Some(
                "Grids with ordered plot ids; style each through ConfigureLinePlot.".into(),
            ),
        }
    }
    fn execute(&self, world: &World, _: &lunco_api_core::ApiValue) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let mut grids: Vec<_> = world.resource::<SignalGrids>().0.iter().collect();
        grids.sort_by_key(|(id, _)| **id);
        Ok(Some(
            api_value!({"grids": grids.into_iter().map(|(id, grid)| api_value!({
            "grid": *id, "title": grid.title.clone(), "columns": grid.columns,
            "plots": grid.plots.iter().map(|id| id.raw()).collect::<Vec<_>>(),
            "sync_time_axes": grid.sync_time_axes,
            "time_span_s": grid_time_span(grid, world.resource::<VisualizationRegistry>(), world.resource::<SignalRegistry>(), None)
        })).collect::<Vec<_>>()}),
        ))
    }
}

register_commands!(on_configure_signal_grid);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_cells_do_not_overlap_and_fill_four_and_sixty_channel_views() {
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        for (columns, count) in [(2, 4), (6, 60)] {
            validate_shape(columns as u32, count).unwrap();
            let rects: Vec<_> = (0..count)
                .map(|i| cell_rect(bounds, columns, count, i, 8.0))
                .collect();
            for (i, rect) in rects.iter().enumerate() {
                assert!(bounds.contains_rect(*rect));
                assert!(rect.width() > 0.0 && rect.height() > 0.0);
                for other in &rects[..i] {
                    assert!(!rect.intersects(*other));
                }
            }
            assert_eq!(rects[0].min, bounds.min);
            assert_eq!(rects.last().unwrap().max, bounds.max);
        }
        for (columns, count) in [(0, 4), (3, 2), (9, 60), (2, 65), (1, 0)] {
            assert!(validate_shape(columns, count).is_err());
        }
    }
}
