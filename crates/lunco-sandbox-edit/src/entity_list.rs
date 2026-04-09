//! Entity list panel — shows all named entities in the scene.
//!
//! Clicking an entity selects it and enables the transform gizmo.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, egui};

use crate::{SelectedEntity, ToolMode};

/// Plugin that registers the entity list panel system.
pub struct EntityListPlugin;

impl Plugin for EntityListPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(bevy_inspector_egui::bevy_egui::EguiPrimaryContextPass, entity_list_panel);
    }
}

/// Renders the entity list panel in the EGUI context.
pub fn entity_list_panel(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedEntity>,
    q_names: Query<(Entity, &Name, Option<&Transform>)>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::Window::new("Scene Entities")
        .resizable(true)
        .default_width(280.0)
        .default_height(400.0)
        .show(ctx, |ui| {
            ui.label("Click to select an entity. Use gizmos to move it.");
            ui.separator();

            // Collect and sort entities by name
            let mut entities: Vec<_> = q_names.iter()
                .map(|(e, name, tf)| (e, name.as_str().to_string(), tf.cloned()))
                .collect();
            entities.sort_by(|a, b| a.1.cmp(&b.1));

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (entity, name, _tf) in &entities {
                    let is_selected = selected.entity == Some(*entity);
                    let button = egui::Button::new(name);
                    let button = if is_selected {
                        button.fill(egui::Color32::DARK_GREEN)
                    } else {
                        button
                    };

                    if ui.add(button).clicked() {
                        selected.entity = Some(*entity);
                        selected.mode = ToolMode::Translate;
                        selected.is_picking_up = false;
                    }
                }
            });
        });
}
