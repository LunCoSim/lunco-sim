//! Entity list panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Lists all named entities alphabetically; clicking one selects it.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::SelectedEntity;

/// Entity list panel — selectable list of scene entities.
pub struct EntityList;

impl WorkbenchPanel for EntityList {
    fn id(&self) -> &str { "entity_list" }
    fn title(&self) -> String { "Entities".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Entity list requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Draw opaque background for this panel
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.label("Click to select an entity. Use gizmos to move it.");
        ui.separator();

        // Collect entity list (read-only, scoped borrow)
        let entities: Vec<(Entity, String)> = world.query::<(Entity, &Name)>().iter(world)
            .map(|(e, name)| (e, name.as_str().to_string()))
            .collect();

        // Sort by name
        let mut sorted: Vec<_> = entities.iter().collect();
        sorted.sort_by(|a, b| a.1.cmp(&b.1));

        // Get current selection (separate borrow)
        let currently_selected = world
            .get_resource::<SelectedEntity>()
            .and_then(|s| s.entity);

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (entity, name) in &sorted {
                let is_selected = currently_selected == Some(*entity);
                let button = egui::Button::new(name);
                let button = if is_selected {
                    button.fill(egui::Color32::DARK_GREEN)
                } else {
                    button
                };

                if ui.add(button).clicked() {
                    // Selection mutation (separate borrow)
                    if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                        selected.entity = Some(*entity);
                    }
                }
            }
        });
    }
}
