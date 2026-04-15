//! MSL Component Palette — click to place components on the Diagram canvas.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::ui::panels::diagram::DiagramState;
use crate::visual_diagram::{
    msl_categories, msl_components_in_category,
};

/// MSL Component Palette panel.
pub struct MSLPalettePanel;

impl WorkbenchPanel for MSLPalettePanel {
    fn id(&self) -> &str { "msl_palette" }
    fn title(&self) -> String { "📦 MSL Library".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(35, 35, 40))
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<DiagramState>().is_none() {
            world.insert_resource(DiagramState::default());
        }

        // Category selection
        let cat_id = egui::Id::new("msl_selected_category");
        let mut selected_cat = ui.memory_mut(|mem| {
            mem.data.get_temp::<String>(cat_id).unwrap_or_else(|| "Electrical/Analog/Basic".to_string())
        });

        ui.label(egui::RichText::new("Click to place on canvas:").size(10.0).color(egui::Color32::GRAY));
        ui.separator();

        let categories = msl_categories();

        // Horizontal category tabs
        ui.horizontal_wrapped(|ui| {
            for cat in &categories {
                let short = cat.split('/').last().unwrap_or(cat);
                let is_sel = cat == &selected_cat;
                if ui.selectable_label(is_sel, short).clicked() {
                    selected_cat = cat.clone();
                    ui.memory_mut(|mem| mem.data.insert_temp(cat_id, cat.clone()));
                }
            }
        });

        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            let components = msl_components_in_category(&selected_cat);
            for comp in &components {
                ui.add_space(2.0);
                if ui.button(format!("{} {}", comp.display_name, comp.name)).clicked() {
                    if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                        state.placement_counter += 1;
                        let x = 80.0 + (state.placement_counter % 4) as f32 * 220.0;
                        let y = 60.0 + (state.placement_counter / 4) as f32 * 180.0;
                        state.add_component(comp.clone(), egui::Pos2::new(x, y));
                    }
                }
            }
        });
    }
}
