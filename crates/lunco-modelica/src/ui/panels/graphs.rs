//! Modelica plot panel — multi-instance host for time-series plots.
//!
//! Each tab is a `ModelicaPlotPanel` instance keyed by `VizId`. The
//! historical singleton "Graphs" tab is now just the *first* instance,
//! auto-spawned at startup with `VizId = DEFAULT_MODELICA_GRAPH` and
//! the title "Graphs". Telemetry checkboxes still bind their signals
//! to that default config; users can open additional plots via
//! `NewPlotPanel` (the `➕` button) and each gets its own
//! `VisualizationConfig` with independent live-signal bindings.
//!
//! 1. Reserves the `modelica_graphs` slot in the bottom dock.
//! 2. Renders a small toolbar (Fit + count).
//! 3. Delegates the plot to [`LinePlot::render_panel_2d`] reading the
//!    [`DEFAULT_MODELICA_GRAPH`]
//!    config.
//!
//! No shadow state, no per-frame syncing — Telemetry writes directly
//! to the same config and the worker pushes samples into the same
//! `SignalRegistry`. Adding multiple plots is a future feature: open
//! a new `VizPanel` instance via `OpenTab { kind: VIZ_PANEL_KIND, .. }`.
//!
//! Experiments overlay state (`ExperimentVisibility`) is currently
//! shared across all plot instances — picked variables show up in
//! every plot's experiments section. Per-panel experiment state is
//! a follow-up; the live-signal split (the more impactful one) is in
//! place.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{InstancePanel, PanelId, PanelSlot};
use lunco_viz::{
    export_signals_to_csv, kinds::line_plot::LinePlot, view::Panel2DCtx, viz::Visualization,
    viz::VizId, SignalRegistry, VisualizationRegistry, VizFitRequests,
};

use crate::ui::viz::{ensure_default_modelica_graph, DEFAULT_MODELICA_GRAPH};

/// Multi-instance kind id. Each instance is a `VizId.0`.
pub const MODELICA_PLOT_KIND: PanelId = PanelId("modelica_plot");

#[derive(Default)]
pub struct ModelicaPlotPanel;

impl InstancePanel for ModelicaPlotPanel {
    fn kind(&self) -> PanelId { MODELICA_PLOT_KIND }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }

    fn title(&self, world: &World, instance: u64) -> String {
        let id = VizId(instance);
        // The default plot keeps the historical "Graphs" name. Other
        // instances use whatever title was set on creation, falling
        // back to "Plot #N" via the registry config.
        if id == DEFAULT_MODELICA_GRAPH {
            return "📈 Graphs".into();
        }
        world
            .get_resource::<VisualizationRegistry>()
            .and_then(|r| r.get(id))
            .map(|cfg| format!("📈 {}", cfg.title))
            .unwrap_or_else(|| format!("📈 Plot #{instance}"))
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64) {
        render_modelica_plot(ui, world, VizId(instance));
    }
}

/// Render body shared by every Modelica plot instance.
///
/// Reads the per-VizId `VisualizationConfig` (live-signal bindings),
/// renders the polished toolbar (live-summary / Fit / CSV / new-plot),
/// then dispatches to the experiments overlay + LinePlot kind.
fn render_modelica_plot(ui: &mut egui::Ui, world: &mut World, viz_id: VizId) {
    // Mark this plot as the active one for global readers (canvas
    // overlay, telemetry, runner auto-pick). Hover wins over
    // render-order: with two plot panels visible side-by-side, the
    // pointer's panel is the "active" one, not whichever rendered
    // last. Falls back to render-order when there's no pointer
    // (boot, headless, key-only navigation) so a fresh tab gets
    // promoted on first frame.
    let panel_rect = ui.max_rect();
    let hovered_here = ui.rect_contains_pointer(panel_rect);
    if let Some(mut active) =
        world.get_resource_mut::<crate::ui::panels::experiments::ActivePlot>()
    {
        if active.0.is_none() || hovered_here {
            active.0 = Some(viz_id);
        }
    }
    // Bootstrap the registry entry for the default graph the first
    // time the panel renders. Other VizIds were created by
    // `NewPlotPanel` and already exist; this branch is a no-op for
    // them.
    let bound_count = {
        let Some(mut registry) = world.get_resource_mut::<VisualizationRegistry>() else {
            ui.label("lunco-viz not installed.");
            return;
        };
        let cfg_opt = if viz_id == DEFAULT_MODELICA_GRAPH {
            Some(ensure_default_modelica_graph(&mut registry).clone())
        } else {
            registry.get(viz_id).cloned()
        };
        let Some(cfg) = cfg_opt else {
            drop(registry);
            ui.label(format!("Plot #{} not found.", viz_id.0));
            return;
        };
        cfg.inputs.len()
    };
    // Per-plot experiment overlay: each tab has its own picked-vars
    // and scrub cursor, so every plot can render the experiments
    // overlay independently.
    let exp_summary =
        crate::ui::panels::experiments::experiments_plot_summary(world, viz_id);
    let has_live = bound_count > 0;
    let has_exp = exp_summary.total_runs > 0;

    // Single shared action header — New / Duplicate / Fit / CSV —
    // rendered once here regardless of which plot body follows.
    // Both bodies (the live LinePlot and the experiments plot) used
    // to grow their own action clusters; the live path's toolbar
    // never had one, so binding a live variable made the tab-copy
    // buttons vanish. One header, one place, every state.
    render_plot_header(ui, world, viz_id, has_live);

    if has_live && !has_exp {
        // Pure live mode keeps the dedicated LinePlot rendering so
        // the X/Y/+add binding picker stays accessible.
        render_line_plot(ui, world, viz_id);
    } else {
        let extras = if has_live {
            collect_live_extras(world, viz_id)
        } else {
            Vec::new()
        };
        crate::ui::panels::experiments::render_experiments_plot_with_extras(
            ui, world, viz_id, &extras,
        );
    }
}

/// The shared action cluster for every Modelica plot tab, drawn once
/// above the plot body. `➕` opens a fresh plot panel, `📄` duplicates
/// this one (same bindings + picked vars), `📐 Fit` queues a one-shot
/// auto-fit via [`VizFitRequests`] (both the LinePlot and experiments
/// bodies drain it), and `💾 CSV` exports live signal histories.
fn render_plot_header(ui: &mut egui::Ui, world: &mut World, viz_id: VizId, has_live: bool) {
    let mut new_plot = false;
    let mut dup = false;
    let mut fit = false;
    let mut csv = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("➕")
                .on_hover_text("New plot panel — opens a fresh tab.")
                .clicked()
            {
                new_plot = true;
            }
            if ui
                .small_button("📄")
                .on_hover_text(
                    "Duplicate this plot — new tab with the same \
                     signal bindings and picked variables.",
                )
                .clicked()
            {
                dup = true;
            }
            if ui
                .small_button("📐 Fit")
                .on_hover_text("Auto-fit axes to data")
                .clicked()
            {
                fit = true;
            }
            if has_live
                && ui
                    .small_button("💾 CSV")
                    .on_hover_text("Export live signal histories to CSV.")
                    .clicked()
            {
                csv = true;
            }
        });
    });
    ui.separator();

    if new_plot {
        world
            .commands()
            .trigger(crate::ui::commands::NewPlotPanel::default());
    }
    if dup {
        world.commands().trigger(crate::ui::commands::NewPlotPanel {
            source: viz_id.0,
            ..Default::default()
        });
    }
    if fit {
        if let Some(mut reqs) = world.get_resource_mut::<VizFitRequests>() {
            reqs.request(viz_id);
        }
    }
    if csv {
        export_graph_to_csv(world, viz_id);
    }
}

/// Build the live-signal overlay used when the Graphs panel has
/// both completed runs *and* live-signal bindings. Reads the same
/// per-VizId `VisualizationConfig` that the LinePlot kind reads, so
/// the curves match what the dedicated live plot would draw (color,
/// label, visibility — minus the X/Y picker UI, which only the
/// LinePlot toolbar exposes).
fn collect_live_extras(
    world: &World,
    viz_id: VizId,
) -> Vec<crate::ui::panels::experiments::PlotExtraLine> {
    let Some(reg) = world.get_resource::<VisualizationRegistry>() else {
        return Vec::new();
    };
    let Some(cfg) = reg.get(viz_id) else {
        return Vec::new();
    };
    let Some(sigs) = world.get_resource::<SignalRegistry>() else {
        return Vec::new();
    };
    cfg.inputs
        .iter()
        .filter(|b| b.role == "y" && b.visible)
        .filter_map(|b| {
            let hist = sigs.scalar_history(&b.source)?;
            if hist.is_empty() {
                return None;
            }
            let points: Vec<[f64; 2]> = hist.iter().map(|s| [s.time, s.value]).collect();
            let color = b
                .color
                .unwrap_or_else(|| lunco_viz::signal::color_for_signal(&b.source.path));
            let label = b.label.clone().unwrap_or_else(|| b.source.path.clone());
            Some(crate::ui::panels::experiments::PlotExtraLine {
                label,
                color: (color.r(), color.g(), color.b()),
                points,
            })
        })
        .collect()
}

fn render_line_plot(ui: &mut egui::Ui, world: &mut World, viz_id: VizId) {
    let config = match world.resource::<VisualizationRegistry>().get(viz_id) {
        Some(c) => c.clone(),
        None => return,
    };
    let viz = LinePlot;
    let mut ctx = Panel2DCtx { ui, world };
    viz.render_panel_2d(&mut ctx, &config);
}

/// Gather the plot's bound signals, pop a native save-file picker,
/// and write a CSV with `time` + one column per signal.
fn export_graph_to_csv(world: &mut World, viz_id: VizId) {
    let (signals, labels) = {
        let Some(reg) = world.get_resource::<VisualizationRegistry>() else { return };
        let Some(cfg) = reg.get(viz_id) else { return };
        let sigs: Vec<_> = cfg.inputs.iter().map(|b| b.source.clone()).collect();
        let labels: Vec<String> = cfg
            .inputs
            .iter()
            .map(|b| b.label.clone().unwrap_or_else(|| b.source.path.clone()))
            .collect();
        (sigs, labels)
    };
    if signals.is_empty() {
        return;
    }

    let csv = {
        let Some(reg) = world.get_resource::<SignalRegistry>() else { return };
        export_signals_to_csv(reg, &signals, &labels)
    };

    let storage = lunco_storage::FileStorage::new();
    let hint = lunco_storage::SaveHint {
        suggested_name: Some(format!("modelica_plot_{}.csv", viz_id.0)),
        start_dir: None,
        filters: vec![lunco_storage::OpenFilter::new("CSV", &["csv"])],
    };
    let handle = match futures_lite::future::block_on(<lunco_storage::FileStorage as lunco_storage::Storage>::pick_save(
        &storage, &hint,
    )) {
        Ok(Some(h)) => h,
        Ok(None) => return,
        Err(e) => {
            if let Some(mut console) =
                world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
            {
                console.error(format!("CSV export: picker failed: {e}"));
            }
            return;
        }
    };

    if let Err(e) = futures_lite::future::block_on(<lunco_storage::FileStorage as lunco_storage::Storage>::write(
        &storage,
        &handle,
        csv.as_bytes(),
    )) {
        if let Some(mut console) =
            world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
        {
            console.error(format!("CSV export: write failed: {e}"));
        }
    } else if let Some(mut console) =
        world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
    {
        let path = match &handle {
            lunco_storage::StorageHandle::File(p) => p.display().to_string(),
            _ => "(handle)".to_string(),
        };
        console.info(format!(
            "Exported {} bytes ({} signals) to {path}",
            csv.len(),
            signals.len()
        ));
    }
}
