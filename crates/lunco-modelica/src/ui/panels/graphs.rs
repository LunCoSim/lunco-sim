//! Graphs panel — time-series plots of Modelica variables.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};

use crate::ui::WorkbenchState;

/// Graphs panel — time-series plots of Modelica variables.
pub struct GraphsPanel;

impl Panel for GraphsPanel {
    fn id(&self) -> PanelId { PanelId("modelica_graphs") }
    fn title(&self) -> String { "📈 Graphs".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Auto-select first ModelicaModel entity if none selected (matches old behavior)
        {
            let needs_select = world.get_resource::<WorkbenchState>()
                .map_or(true, |s| s.selected_entity.is_none());
            if needs_select {
                type Q = bevy::ecs::query::QueryState<Entity, bevy::ecs::query::With<crate::ModelicaModel>>;
                let mut query_state = Q::new(world);
                if let Some(entity) = query_state.iter(world).next() {
                    if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                        s.selected_entity = Some(entity);
                    }
                }
            }
        }

        let Some(state) = world.get_resource::<WorkbenchState>() else { return };

        let Some(entity) = state.selected_entity else {
            ui.label("No model selected.");
            return;
        };

        // Toolbar row — always visible, even when there's nothing to
        // plot. Keeps the Auto-Fit affordance local to the graph
        // instead of hiding it on the Telemetry panel where users
        // couldn't find it. "📐" matches the "zoom-fit" convention
        // from CAD / Figma.
        let mut fit_requested = false;
        ui.horizontal(|ui| {
            if ui
                .small_button("📐 Fit")
                .on_hover_text("Auto-scale axes to current data (F)")
                .clicked()
            {
                fit_requested = true;
            }
            ui.separator();
            let plotted_count = state.plotted_variables.len();
            ui.label(
                egui::RichText::new(format!("{plotted_count} variable(s) plotted"))
                    .size(10.0)
                    .color(egui::Color32::GRAY),
            );
        });
        ui.separator();
        if fit_requested {
            if let Some(mut st) = world.get_resource_mut::<WorkbenchState>() {
                st.plot_auto_fit = true;
            }
            return; // redraw next frame with the flag set
        }

        let Some(entity_history) = state.history.get(&entity).cloned() else {
            ui.label("No data to plot.");
            return;
        };

        let plotted: Vec<String> = state.plotted_variables.iter().cloned().collect();
        let auto_fit = state.plot_auto_fit;

        // Pull descriptions from the linked ModelicaModel so legend
        // entries can carry tooltip-style metadata. Legend itself
        // shows just the variable name; callers hover in Telemetry
        // for the description.
        let _descriptions = world
            .get::<crate::ModelicaModel>(entity)
            .map(|m| m.descriptions.clone())
            .unwrap_or_default();

        let _ = state;

        if plotted.is_empty() {
            ui.label("No variables selected for plotting.");
            ui.label("Go to Telemetry and check variables to plot.");
            return;
        }

        let colors = [
            egui::Color32::from_rgb(80, 160, 255),
            egui::Color32::from_rgb(255, 120, 80),
            egui::Color32::from_rgb(80, 220, 120),
            egui::Color32::from_rgb(255, 220, 80),
            egui::Color32::from_rgb(200, 120, 255),
            egui::Color32::from_rgb(120, 200, 200),
            egui::Color32::from_rgb(230, 120, 180),
            egui::Color32::from_rgb(180, 230, 100),
        ];

        // Size plot to fill its tile's bounded rect. Legend anchored
        // in the upper-right corner (matches Dymola / Matplotlib
        // default); egui_plot's legend is click-to-toggle so users
        // can hide individual traces without unchecking in Telemetry.
        let tile_rect = ui.max_rect();
        let mut plot = Plot::new("modelica_plot")
            .view_aspect(3.0)
            .width(tile_rect.width())
            .height(tile_rect.height())
            .include_y(0.0)
            .legend(
                Legend::default()
                    .position(Corner::RightTop)
                    .background_alpha(0.7),
            );

        if auto_fit {
            plot = plot.auto_bounds(egui::emath::Vec2b::new(true, true));
            if let Some(mut st) = world.get_resource_mut::<WorkbenchState>() {
                st.plot_auto_fit = false;
            }
        }

        let lines: Vec<Line> = plotted.iter().enumerate()
            .filter_map(|(i, var_name)| {
                entity_history.get(var_name).map(|history| {
                    let pts: Vec<[f64; 2]> = history.iter().map(|p| [p[0], p[1]]).collect();
                    // `Line::new(name, ...)` feeds the legend — the
                    // name shown there is the variable name. Click
                    // on a legend row to toggle that trace.
                    Line::new(var_name.clone(), PlotPoints::new(pts))
                        .color(colors[i % colors.len()])
                })
            })
            .collect();

        plot.show(ui, |plot_ui| {
            for line in lines {
                plot_ui.line(line);
            }
        });
    }
}
