//! Modelica Graphs panel — workspace-layout entry point for the
//! singleton "Modelica" plot.
//!
//! All state and rendering live in `lunco-viz`; this panel is the
//! workbench-side wiring that:
//!
//! 1. Reserves the `modelica_graphs` slot in the bottom dock.
//! 2. Renders a small toolbar (Fit + count).
//! 3. Delegates the plot to [`LinePlot::render_panel_2d`] reading the
//!    [`DEFAULT_MODELICA_GRAPH`](crate::ui::viz::DEFAULT_MODELICA_GRAPH)
//!    config.
//!
//! No shadow state, no per-frame syncing — Telemetry writes directly
//! to the same config and the worker pushes samples into the same
//! `SignalRegistry`. Adding multiple plots is a future feature: open
//! a new `VizPanel` instance via `OpenTab { kind: VIZ_PANEL_KIND, .. }`.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use lunco_viz::{
    kinds::line_plot::LinePlot, view::Panel2DCtx, viz::Visualization, SignalRegistry,
    VisualizationRegistry, VizFitRequests,
};

use crate::ui::viz::{ensure_default_modelica_graph, DEFAULT_MODELICA_GRAPH};

pub struct GraphsPanel;

impl Panel for GraphsPanel {
    fn id(&self) -> PanelId { PanelId("modelica_graphs") }
    fn title(&self) -> String { "📈 Graphs".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Take a cheap snapshot of everything the toolbar needs so we
        // can also render the plot in the same frame without
        // re-borrowing resources.
        let (bound_count, time_min, time_max, sample_total) = {
            let Some(mut registry) = world.get_resource_mut::<VisualizationRegistry>()
            else {
                ui.label("lunco-viz not installed.");
                return;
            };
            let cfg = ensure_default_modelica_graph(&mut registry);
            let count = cfg.inputs.len();
            let sources: Vec<_> = cfg.inputs.iter().map(|b| b.source.clone()).collect();
            drop(registry);

            // Time-range readout across all bound signals — the most
            // useful single number on a time-series plot. Falls back
            // to NaN when no data, handled by the label below.
            let (mut t_min, mut t_max, mut total) = (f64::INFINITY, f64::NEG_INFINITY, 0usize);
            if let Some(sigs) = world.get_resource::<SignalRegistry>() {
                for src in &sources {
                    if let Some(hist) = sigs.scalar_history(src) {
                        if let (Some(first), Some(last)) =
                            (hist.samples.front(), hist.samples.back())
                        {
                            t_min = t_min.min(first.time);
                            t_max = t_max.max(last.time);
                        }
                        total += hist.len();
                    }
                }
            }
            (count, t_min, t_max, total)
        };

        // Toolbar — data on the left, controls on the right. The
        // Fit button is a compact icon so the row is actually useful
        // for telemetry readouts, not empty space around one button.
        let mut fit_clicked = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{bound_count} var"))
                    .size(10.0)
                    .color(egui::Color32::GRAY),
            );
            if time_min.is_finite() && time_max.is_finite() {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "t: {time_min:.2} → {time_max:.2} s  ({:.2} s window)",
                        time_max - time_min
                    ))
                    .size(10.0)
                    .color(egui::Color32::GRAY),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{sample_total} samples"))
                        .size(10.0)
                        .color(egui::Color32::DARK_GRAY),
                );
            }

            // Right-aligned controls — reserve just enough width for
            // the icons and push everything else left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let new_plot = ui
                    .small_button("➕")
                    .on_hover_text(
                        "New plot panel (➕) — opens a fresh tab so you can plot \
                         a different signal set side-by-side with this one",
                    );
                if new_plot.clicked() {
                    world
                        .commands()
                        .trigger(crate::ui::commands::NewPlotPanel::default());
                }
                let fit = ui
                    .small_button("📐")
                    .on_hover_text("Auto-fit (📐) — rescale axes to current data");
                if fit.clicked() {
                    fit_clicked = true;
                }
            });
        });
        ui.separator();
        if fit_clicked {
            if let Some(mut requests) = world.get_resource_mut::<VizFitRequests>() {
                requests.request(DEFAULT_MODELICA_GRAPH);
            }
        }

        if bound_count == 0 {
            ui.label("No variables selected for plotting.");
            ui.label("Go to Telemetry and check variables to plot.");
            return;
        }

        let config = match world.resource::<VisualizationRegistry>().get(DEFAULT_MODELICA_GRAPH) {
            Some(c) => c.clone(),
            None => return,
        };
        let viz = LinePlot;
        let mut ctx = Panel2DCtx { ui, world };
        viz.render_panel_2d(&mut ctx, &config);
    }
}
