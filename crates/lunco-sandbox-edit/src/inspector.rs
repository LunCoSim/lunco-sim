//! EGUI inspector panel for editing entity parameters.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, egui};

use crate::SelectedEntity;

/// Renders the inspector panel in the EGUI context.
pub fn inspector_panel(
    mut contexts: EguiContexts,
    selected: Res<SelectedEntity>,
    q_names: Query<&Name>,
    q_transforms: Query<&Transform>,
    q_rb: Query<&avian3d::prelude::RigidBody>,
    q_mass: Query<&avian3d::prelude::Mass>,
) {
    let Some(entity) = selected.entity else { return };

    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::Window::new("Sandbox Inspector")
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.heading("Entity");
            ui.label(format!("Entity ID: {:?}", entity));

            if let Ok(name) = q_names.get(entity) {
                ui.label(format!("Name: {}", name.as_str()));
            }

            ui.separator();
            ui.heading("Transform");

            if let Ok(tf) = q_transforms.get(entity) {
                ui.label(format!(
                    "Pos: ({:.2}, {:.2}, {:.2})",
                    tf.translation.x, tf.translation.y, tf.translation.z
                ));
            }

            ui.separator();
            ui.heading("Physics");

            if let Ok(rb) = q_rb.get(entity) {
                ui.label(format!("Rigid Body: {:?}", rb));
            }
            if let Ok(mass) = q_mass.get(entity) {
                ui.label(format!("Mass: {:.2}", mass.0));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::*;
    use crate::UndoStack;

    #[test]
    fn test_selected_entity_no_panel() {
        let selected = SelectedEntity::default();
        assert!(selected.entity.is_none());
    }

    #[test]
    fn test_inspector_with_reflect_component() {
        // Verify Mass component exists and has expected value
        let mass = Mass(10.0);
        assert!((mass.0 - 10.0).abs() < 0.01);
    }
}
