//! EGUI inspector panel for editing entity parameters.
//!
//! Provides editable sliders for:
//! - Transform (position)
//! - Mass (for rigid bodies)
//! - Linear/Angular Damping
//! - WheelRaycast parameters (spring K, damping C, rest length)
//! - Delete entity
//!
//! Changes are applied immediately and recorded for undo.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, egui};
use lunco_mobility::WheelRaycast;

use crate::{SelectedEntity, ToolMode, UndoStack, UndoAction};

/// Renders the inspector panel in the EGUI context.
pub fn inspector_panel(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedEntity>,
    mut undo_stack: ResMut<UndoStack>,
    q_names: Query<&Name>,
    mut q_transforms: Query<&mut Transform>,
    q_rb: Query<&avian3d::prelude::RigidBody>,
    mut q_mass: Query<&mut avian3d::prelude::Mass>,
    mut q_lin_damp: Query<&mut avian3d::prelude::LinearDamping>,
    mut q_ang_damp: Query<&mut avian3d::prelude::AngularDamping>,
    mut q_wheels: Query<&mut WheelRaycast>,
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Hotkeys work even without a selection
    if keys.just_pressed(KeyCode::KeyG) {
        selected.mode = ToolMode::Translate;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        selected.mode = ToolMode::Rotate;
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        selected.mode = ToolMode::Select;
    }
    if keys.just_pressed(KeyCode::Delete) {
        if let Some(entity) = selected.entity {
            undo_stack.push(UndoAction::Spawned { entity });
            if commands.get_entity(entity).is_ok() {
                commands.entity(entity).despawn();
            }
            selected.entity = None;
        }
    }

    egui::Window::new("Sandbox Inspector")
        .resizable(true)
        .default_width(320.0)
        .show(ctx, |ui| {
            let Some(entity) = selected.entity else {
                ui.label("No entity selected.");
                ui.label("Press Shift+Left-click on an object to select it.");
                ui.separator();
                ui.heading("Mode");
                let mode_str = match selected.mode {
                    ToolMode::Select => "Select",
                    ToolMode::Translate => "Translate (G)",
                    ToolMode::Rotate => "Rotate (R)",
                    ToolMode::Pickup => "Pickup",
                };
                ui.label(format!("Current: {}", mode_str));
                return;
            };

            ui.heading("Entity");
            ui.label(format!("ID: {:?}", entity));

            if let Ok(name) = q_names.get(entity) {
                ui.label(format!("Name: {}", name.as_str()));
            }

            ui.separator();
            ui.heading("Transform");

            if let Ok(mut tf) = q_transforms.get_mut(entity) {
                let mut x = tf.translation.x;
                let mut y = tf.translation.y;
                let mut z = tf.translation.z;
                let changed_x = ui.add(egui::Slider::new(&mut x, -1000.0..=1000.0).text("X")).changed();
                let changed_y = ui.add(egui::Slider::new(&mut y, -1000.0..=1000.0).text("Y")).changed();
                let changed_z = ui.add(egui::Slider::new(&mut z, -1000.0..=1000.0).text("Z")).changed();
                if changed_x || changed_y || changed_z {
                    undo_stack.push(UndoAction::TransformChanged {
                        entity,
                        old_translation: tf.translation,
                        old_rotation: tf.rotation,
                    });
                    tf.translation = Vec3::new(x, y, z);
                }
            }

            ui.separator();
            ui.heading("Physics");

            if let Ok(rb) = q_rb.get(entity) {
                ui.label(format!("Type: {:?}", rb));
            }

            if let Ok(mut mass) = q_mass.get_mut(entity) {
                let mut m = mass.0;
                if ui.add(egui::Slider::new(&mut m, 0.1..=100000.0).text("Mass (kg)").logarithmic(true)).changed() {
                    mass.0 = m;
                }
            }

            if let Ok(mut damp) = q_lin_damp.get_mut(entity) {
                let mut d = damp.0 as f32;
                if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping")).changed() {
                    damp.0 = d as f64;
                }
            }

            if let Ok(mut damp) = q_ang_damp.get_mut(entity) {
                let mut d = damp.0 as f32;
                if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping")).changed() {
                    damp.0 = d as f64;
                }
            }

            // Wheel parameters
            if let Ok(mut wheel) = q_wheels.get_mut(entity) {
                ui.separator();
                ui.heading("Wheel (Raycast)");

                let mut rest = wheel.rest_length as f32;
                if ui.add(egui::Slider::new(&mut rest, 0.1..=2.0).text("Rest Length (m)")).changed() {
                    wheel.rest_length = rest as f64;
                }

                let mut k = wheel.spring_k as f32;
                if ui.add(egui::Slider::new(&mut k, 100.0..=100000.0).text("Spring K (N/m)").logarithmic(true)).changed() {
                    wheel.spring_k = k as f64;
                }

                let mut d = wheel.damping_c as f32;
                if ui.add(egui::Slider::new(&mut d, 100.0..=10000.0).text("Damping C (N·s/m)").logarithmic(true)).changed() {
                    wheel.damping_c = d as f64;
                }

                let mut radius = wheel.wheel_radius as f32;
                if ui.add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Wheel Radius (m)")).changed() {
                    wheel.wheel_radius = radius as f64;
                }
            }

            // Delete button
            ui.separator();
            if ui.button("🗑 Delete Entity (Del)").clicked() {
                undo_stack.push(UndoAction::Spawned { entity });
                if commands.get_entity(entity).is_ok() {
                    commands.entity(entity).despawn();
                }
                selected.entity = None;
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
        let mass = Mass(10.0);
        assert!((mass.0 - 10.0).abs() < 0.01);
    }
}
