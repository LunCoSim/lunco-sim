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
use lunco_workbench::{InstancePanel, PanelCtx, PanelId, PanelSlot};
use lunco_viz::{
    kinds::line_plot::LinePlot, view::Panel2DCtx, viz::Visualization,
    viz::VizId, SignalRegistry, VisualizationRegistry, VizFitRequests,
};
use lunco_experiments::{ExperimentId, ExperimentRegistry};
use crate::ui::panels::experiments::PlotPanelStates;

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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        render_modelica_plot(ui, ctx, VizId(instance));
    }
}

/// Render body shared by every Modelica plot instance.
///
/// Reads the per-VizId `VisualizationConfig` (live-signal bindings),
/// renders the polished toolbar (live-summary / Fit / CSV / new-plot),
/// then dispatches to the experiments overlay + LinePlot kind.
fn render_modelica_plot(ui: &mut egui::Ui, ctx: &mut PanelCtx, viz_id: VizId) {
    // Mark this plot as the active one for global readers (canvas
    // overlay, telemetry, runner auto-pick). Hover wins over
    // render-order: with two plot panels visible side-by-side, the
    // pointer's panel is the "active" one, not whichever rendered
    // last. Falls back to render-order when there's no pointer
    // (boot, headless, key-only navigation) so a fresh tab gets
    // promoted on first frame. The write is pure global-state intent
    // (this panel never reads ActivePlot back this frame), so it is
    // deferred to run after paint.
    let panel_rect = ui.max_rect();
    let hovered_here = ui.rect_contains_pointer(panel_rect);
    ctx.defer(move |world| {
        if let Some(mut active) =
            world.get_resource_mut::<crate::ui::panels::experiments::ActivePlot>()
        {
            if active.0.is_none() || hovered_here {
                active.0 = Some(viz_id);
            }
        }
    });
    // Bootstrap the registry entry for the default graph the first
    // time the panel renders. Other VizIds were created by
    // `NewPlotPanel` and already exist; this branch is a no-op for
    // them. The bootstrap mutates the registry and we need the
    // resulting binding count *this* frame to pick the body, so this
    // is a mutate-then-read — use `resource_scope`.
    let bound_count = match ctx.resource_scope::<VisualizationRegistry, _>(|_ctx, registry| {
        if viz_id == DEFAULT_MODELICA_GRAPH {
            Some(ensure_default_modelica_graph(registry).inputs.len())
        } else {
            registry.get(viz_id).map(|cfg| cfg.inputs.len())
        }
    }) {
        None => {
            ui.label("lunco-viz not installed.");
            return;
        }
        Some(None) => {
            ui.label(format!("Plot #{} not found.", viz_id.0));
            return;
        }
        Some(Some(n)) => n,
    };
    // Per-plot experiment overlay: each tab has its own picked-vars
    // and scrub cursor, so every plot can render the experiments
    // overlay independently. We only need "are there runs?" to pick the
    // body — the experiments render recomputes its own counts (CQ-207).
    let has_live = bound_count > 0;
    let has_exp = crate::ui::panels::experiments::has_experiment_runs(ctx);

    if has_live && !has_exp {
        // Pure live mode keeps its own one-line action header above the
        // dedicated LinePlot (which owns the X/Y/+add binding picker and
        // its own log-Y toggle).
        render_plot_header(ui, ctx, viz_id);
        render_line_plot(ui, ctx, viz_id);
    } else {
        // The experiments body draws the action buttons (New / Dup / Fit /
        // CSV) and the log-Y toggle inline on its Variables/Runs row, so
        // the whole toolbar is a single line — no separate header here.
        let extras = if has_live {
            collect_live_extras(ctx, viz_id)
        } else {
            Vec::new()
        };
        crate::ui::panels::experiments::render_experiments_plot_with_extras(
            ui, ctx, viz_id, &extras,
        );
    }
}

/// The pure-live action header: a single right-aligned button row above
/// the dedicated LinePlot. The experiments body doesn't use this — it
/// renders [`plot_action_buttons`] inline on its own pickers row.
fn render_plot_header(ui: &mut egui::Ui, ctx: &mut PanelCtx, viz_id: VizId) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            plot_action_buttons(ui, ctx, viz_id);
        });
    });
    ui.separator();
}

/// The shared action cluster for every Modelica plot tab: `➕` opens a
/// fresh plot panel, `📄` duplicates this one (same bindings + picked
/// vars), `📐 Fit` queues a one-shot auto-fit via [`VizFitRequests`]
/// (both the LinePlot and experiments bodies drain it), and `💾 CSV`
/// exports the plot's curves. Renders in the caller's current layout
/// direction (the callers use right-to-left, so `➕` lands rightmost).
pub(crate) fn plot_action_buttons(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    viz_id: VizId,
) {
    let mut new_plot = false;
    let mut dup = false;
    let mut fit = false;
    let mut csv = false;
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
    if ui
        .small_button("💾 CSV")
        .on_hover_text("Export the plot's curves to CSV.")
        .clicked()
    {
        csv = true;
    }

    // All four actions are user-intent mutations (trigger / resource
    // write / file IO) that don't affect this frame's paint, so defer
    // them to run after the UI is drawn.
    if new_plot {
        ctx.defer(move |world| {
            world
                .commands()
                .trigger(crate::ui::commands::NewPlotPanel::default());
        });
    }
    if dup {
        ctx.defer(move |world| {
            world.commands().trigger(crate::ui::commands::NewPlotPanel {
                source: viz_id.0,
                ..Default::default()
            });
        });
    }
    if fit {
        ctx.defer(move |world| {
            if let Some(mut reqs) = world.get_resource_mut::<VizFitRequests>() {
                reqs.request(viz_id);
            }
        });
    }
    if csv {
        ctx.defer(move |world| export_graph_to_csv(world, viz_id));
    }
}

/// Build the live-signal overlay used when the Graphs panel has
/// both completed runs *and* live-signal bindings. Reads the same
/// per-VizId `VisualizationConfig` that the LinePlot kind reads, so
/// the curves match what the dedicated live plot would draw (color,
/// label, visibility — minus the X/Y picker UI, which only the
/// LinePlot toolbar exposes).
fn collect_live_extras(
    ctx: &PanelCtx,
    viz_id: VizId,
) -> Vec<crate::ui::panels::experiments::PlotExtraLine> {
    let Some(reg) = ctx.resource::<VisualizationRegistry>() else {
        return Vec::new();
    };
    let Some(cfg) = reg.get(viz_id) else {
        return Vec::new();
    };
    let Some(sigs) = ctx.resource::<SignalRegistry>() else {
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

fn render_line_plot(ui: &mut egui::Ui, ctx: &mut PanelCtx, viz_id: VizId) {
    let config = match ctx.resource::<VisualizationRegistry>().and_then(|r| r.get(viz_id)) {
        Some(c) => c.clone(),
        None => return,
    };
    let viz = LinePlot;
    let mut p2d = Panel2DCtx { ui, wb: ctx };
    viz.render_panel_2d(&mut p2d, &config);
}

/// Gather the plot's bound signals, pop a native save-file picker,
/// and write a CSV with `time` + one column per signal.
fn export_graph_to_csv(world: &mut World, viz_id: VizId) {
    struct Column {
        label: String,
        data: Vec<(f64, f64)>,
    }
    let mut columns: Vec<Column> = Vec::new();
    let mut all_times: Vec<f64> = Vec::new();

    // 1. Collect live signals from SignalRegistry
    {
        let reg = world.get_resource::<lunco_viz::SignalRegistry>();
        let viz_reg = world.get_resource::<VisualizationRegistry>();
        if let (Some(reg), Some(viz_reg)) = (reg, viz_reg) {
            if let Some(cfg) = viz_reg.get(viz_id) {
                // If "Interactive Live" is hidden in experiments, skip live
                // signals if they are Modelica signals.
                let show_live = world
                    .get_resource::<PlotPanelStates>()
                    .map(|s| s.is_visible(viz_id, ExperimentId::live()))
                    .unwrap_or(true);

                if show_live {
                    for binding in &cfg.inputs {
                        if let Some(hist) = reg.scalar_history(&binding.source) {
                            let label = binding
                                .label
                                .clone()
                                .unwrap_or_else(|| format!("Live · {}", binding.source.path));
                            let mut data = Vec::new();
                            for s in &hist.samples {
                                all_times.push(s.time);
                                data.push((s.time, s.value));
                            }
                            columns.push(Column { label, data });
                        }
                    }
                }
            }
        }
    }

    // 2. Collect visible experiment curves
    {
        let reg = world.get_resource::<ExperimentRegistry>();
        let states = world.get_resource::<PlotPanelStates>();
        if let (Some(reg), Some(states)) = (reg, states) {
            let visible = states.visible(viz_id);
            let picked = states.picked(viz_id);
            let doc_id = crate::ui::doc_pin::resolved_experiments_doc(world);
            let twin = doc_id.map(crate::ui::doc_pin::twin_id_for_doc);

            if let Some(twin) = twin {
                for exp in reg.list_for_twin(&twin) {
                    if !visible.contains(&exp.id) {
                        continue;
                    }
                    if let Some(result) = &exp.result {
                        for var in &picked {
                            if let Some(series) = result.series.get(var) {
                                let label = format!("{} · {}", exp.name, var);
                                let mut data = Vec::new();
                                for (i, &t) in result.times.iter().enumerate() {
                                    if let Some(&v) = series.get(i) {
                                        if v.is_finite() {
                                            all_times.push(t);
                                            data.push((t, v));
                                        }
                                    }
                                }
                                columns.push(Column { label, data });
                            }
                        }
                    }
                }
            }
        }
    }

    if columns.is_empty() {
        return;
    }

    // 3. Flatten into unified CSV rows with forward-filling
    let mut csv = String::from("time");
    for col in &columns {
        csv.push(',');
        // Escape label
        if col.label.contains(',') || col.label.contains('"') || col.label.contains('\n') {
            csv.push('"');
            csv.push_str(&col.label.replace('"', "\"\""));
            csv.push('"');
        } else {
            csv.push_str(&col.label);
        }
    }
    csv.push('\n');

    // Build the master time axis.
    all_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    all_times.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);

    let mut cursors = vec![0usize; columns.len()];
    let mut last_val = vec![Option::<f64>::None; columns.len()];

    for t in all_times {
        csv.push_str(&format!("{t}"));
        for (i, col) in columns.iter().enumerate() {
            while cursors[i] < col.data.len() && col.data[cursors[i]].0 <= t + f64::EPSILON {
                last_val[i] = Some(col.data[cursors[i]].1);
                cursors[i] += 1;
            }
            csv.push(',');
            if let Some(v) = last_val[i] {
                csv.push_str(&format!("{v}"));
            }
        }
        csv.push('\n');
    }

    let storage = lunco_storage::FileStorage::new();
    let hint = lunco_workbench::picker::SaveHint {
        suggested_name: Some(format!("modelica_plot_{}.csv", viz_id.0)),
        start_dir: None,
        filters: vec![lunco_workbench::picker::OpenFilter::new("CSV", &["csv"])],
    };
    let Some(handle) = lunco_workbench::picker::pick_save_blocking(&hint) else {
        return; // user cancelled the save dialog
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
            "Exported {} bytes ({} columns) to {path}",
            csv.len(),
            columns.len()
        ));
    }
}
