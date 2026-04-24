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
    export_signals_to_csv, kinds::line_plot::LinePlot, view::Panel2DCtx, viz::Visualization,
    SignalRegistry, VisualizationRegistry, VizFitRequests,
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
        let mut export_csv_clicked = false;
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
                let csv = ui
                    .small_button("💾 CSV")
                    .on_hover_text(
                        "Export CSV — save the plot's signal histories to a CSV file \
                         (time column + one column per bound signal; forward-filled at \
                         union timestamps)",
                    );
                if csv.clicked() {
                    export_csv_clicked = true;
                }
            });
        });
        ui.separator();
        if fit_clicked {
            if let Some(mut requests) = world.get_resource_mut::<VizFitRequests>() {
                requests.request(DEFAULT_MODELICA_GRAPH);
            }
        }
        if export_csv_clicked {
            export_default_graph_to_csv(world);
        }

        // Inline input controls — operator-style HMI on the plot
        // panel. Lets the user nudge an actuator (`valve.opening`,
        // ...) and watch the response without leaving the graph.
        // Mirrors the Telemetry "Inputs" row, with the same MLS §4.8
        // `min`/`max` clamping via the `parameter_bounds` map. We
        // run our own short query for inputs/bounds rather than
        // duplicating the Telemetry rendering code; this row is
        // intentionally compact.
        render_inline_input_controls(ui, world);

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

/// Render a single horizontal row of `Name [DragValue]` widgets,
/// one per input on the active model. Edits flow into the
/// `ModelicaModel.inputs` map; the worker forwards them to the
/// stepper via `set_input` on the next sync, exactly like the
/// Telemetry panel — we just put them in front of the user
/// who's already watching the plot. Bounds (`min`/`max`) clamp
/// the DragValue using the same leaf-name fallback the Telemetry
/// panel uses, so `valve.opening(min=0,max=100)` clamps to 0..100.
fn render_inline_input_controls(ui: &mut egui::Ui, world: &mut World) {
    use crate::ModelicaModel;

    // Pick the active model — first ModelicaModel entity, matching
    // Telemetry's auto-select behaviour. Snapshot its inputs +
    // bounds so we can render and write back without holding a
    // borrow across the closure.
    let snap: Option<(Entity, Vec<(String, f64)>, std::collections::HashMap<String, (Option<f64>, Option<f64>)>)> = {
        let mut q = world.query::<(Entity, &ModelicaModel)>();
        let mut iter = q.iter(world);
        iter.next().map(|(entity, model)| {
            let mut inputs: Vec<(String, f64)> =
                model.inputs.iter().map(|(k, v)| (k.clone(), *v)).collect();
            inputs.sort_by(|a, b| a.0.cmp(&b.0));
            (entity, inputs, model.parameter_bounds.clone())
        })
    };
    let Some((entity, inputs, bounds)) = snap else {
        return;
    };
    if inputs.is_empty() {
        return;
    }

    let mut edits: Vec<(String, f64)> = Vec::new();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Inputs:")
                .size(11.0)
                .color(egui::Color32::GRAY),
        );
        for (name, mut value) in inputs {
            // Compact label — show the leaf name, full path on hover.
            let leaf = name.rsplit('.').next().unwrap_or(&name).to_string();
            ui.label(
                egui::RichText::new(format!("{leaf}:"))
                    .size(11.0),
            )
            .on_hover_text(name.clone());
            // Same lookup_bounds-style fallback as Telemetry: try
            // qualified name first, then leaf — bounds are keyed by
            // leaf in the AST extractor.
            let (mn, mx) = bounds
                .get(&name)
                .copied()
                .or_else(|| {
                    let leaf_key = name.rsplit('.').next().unwrap_or(&name);
                    bounds.get(leaf_key).copied()
                })
                .unwrap_or((None, None));
            let dv = egui::DragValue::new(&mut value)
                .speed(0.5)
                .fixed_decimals(2)
                .range(
                    mn.unwrap_or(f64::NEG_INFINITY)
                        ..=mx.unwrap_or(f64::INFINITY),
                );
            if ui.add(dv).changed() {
                edits.push((name.clone(), value));
            }
        }
    });
    ui.separator();

    // Write changes back. Done outside the rendering loop so
    // we can re-borrow the model mutably without fighting the
    // immutable iteration.
    if edits.is_empty() {
        return;
    }
    if let Ok(mut model) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
        for (name, value) in edits {
            if let Some(slot) = model.inputs.get_mut(&name) {
                *slot = value;
            }
        }
    }
}

/// Gather the default plot's bound signals, pop a native save-file
/// picker, and write a CSV with `time` + one column per signal.
///
/// Goes through `lunco_storage::FileStorage` so the same call site
/// works when an OPFS / IndexedDB backend lands for wasm. Cancelling
/// the picker is a silent no-op; write errors go to the console.
fn export_default_graph_to_csv(world: &mut World) {
    let (signals, labels) = {
        let Some(reg) = world.get_resource::<VisualizationRegistry>() else { return };
        let Some(cfg) = reg.get(DEFAULT_MODELICA_GRAPH) else { return };
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
        suggested_name: Some("modelica_signals.csv".to_string()),
        start_dir: None,
        filters: vec![lunco_storage::OpenFilter::new("CSV", &["csv"])],
    };
    let handle = match <lunco_storage::FileStorage as lunco_storage::Storage>::pick_save(
        &storage, &hint,
    ) {
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

    if let Err(e) = <lunco_storage::FileStorage as lunco_storage::Storage>::write(
        &storage,
        &handle,
        csv.as_bytes(),
    ) {
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
