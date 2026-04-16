//! Inspector panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Provides editable sliders for transform, physics, and wheel parameters.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use lunco_mobility::WheelRaycast;

use crate::{SelectedEntity, UndoStack, UndoAction};

/// Inspector panel — editable entity parameters.
pub struct Inspector;

impl Panel for Inspector {
    fn id(&self) -> PanelId { PanelId("sandbox_inspector") }
    fn title(&self) -> String { "Inspector".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Draw opaque background for this panel
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        // Delete hotkey
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(entity) = world.get_resource::<SelectedEntity>().and_then(|s| s.entity) {
                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                    undo.push(UndoAction::Spawned { entity });
                }
                if world.get_entity(entity).is_ok() {
                    world.commands().entity(entity).despawn();
                }
                if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                    selected.entity = None;
                }
                return;
            }
        }

        ui.heading("Inspector");

        // Get current selection
        let Some(entity) = world.get_resource::<SelectedEntity>().and_then(|s| s.entity) else {
            ui.label("No entity selected.");
            ui.label("Press Shift+Left-click on an object to select it.");
            return;
        };

        ui.label(format!("ID: {entity:?}"));

        // Name (read-only)
        if let Ok(name) = world.query::<&Name>().get(world, entity) {
            ui.label(format!("Name: {}", name.as_str()));
        }

        ui.separator();
        ui.heading("Transform");

        // Transform — needs undo tracking, so read then mutate
        if let Some((old_tf, new_vals)) = world.query::<&Transform>().get(world, entity).ok().map(|tf| {
            (
                (tf.translation, tf.rotation),
                (tf.translation.x, tf.translation.y, tf.translation.z),
            )
        }) {
            let mut x = new_vals.0;
            let mut y = new_vals.1;
            let mut z = new_vals.2;
            let changed = ui.add(egui::Slider::new(&mut x, -1000.0..=1000.0).text("X")).changed()
                | ui.add(egui::Slider::new(&mut y, -1000.0..=1000.0).text("Y")).changed()
                | ui.add(egui::Slider::new(&mut z, -1000.0..=1000.0).text("Z")).changed();
            if changed {
                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                    undo.push(UndoAction::TransformChanged { entity, old_translation: old_tf.0, old_rotation: old_tf.1 });
                }
                if let Ok(mut tf) = world.query::<&mut Transform>().get_mut(world, entity) {
                    tf.translation = Vec3::new(x, y, z);
                }
            }
        }

        ui.separator();
        ui.heading("Physics");

        // Rigid body type (read-only)
        if let Ok(rb) = world.query::<&avian3d::prelude::RigidBody>().get(world, entity) {
            ui.label(format!("Type: {rb:?}"));
        }

        // Mass
        if let Ok(current) = world.query::<&avian3d::prelude::Mass>().get(world, entity) {
            let mut m = current.0;
            if ui.add(egui::Slider::new(&mut m, 0.1..=100000.0).text("Mass (kg)").logarithmic(true)).changed() {
                if let Ok(mut mass) = world.query::<&mut avian3d::prelude::Mass>().get_mut(world, entity) {
                    mass.0 = m;
                }
            }
        }

        // Linear damping
        if let Ok(current) = world.query::<&avian3d::prelude::LinearDamping>().get(world, entity) {
            let mut d = current.0 as f32;
            if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping")).changed() {
                if let Ok(mut damp) = world.query::<&mut avian3d::prelude::LinearDamping>().get_mut(world, entity) {
                    damp.0 = d as f64;
                }
            }
        }

        // Angular damping
        if let Ok(current) = world.query::<&avian3d::prelude::AngularDamping>().get(world, entity) {
            let mut d = current.0 as f32;
            if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping")).changed() {
                if let Ok(mut damp) = world.query::<&mut avian3d::prelude::AngularDamping>().get_mut(world, entity) {
                    damp.0 = d as f64;
                }
            }
        }

        // Wheel parameters
        if let Ok(current) = world.query::<&WheelRaycast>().get(world, entity) {
            ui.separator();
            ui.heading("Wheel (Raycast)");

            let mut rest = current.rest_length as f32;
            let mut k = current.spring_k as f32;
            let mut d = current.damping_c as f32;
            let mut radius = current.wheel_radius as f32;

            let rest_changed = ui.add(egui::Slider::new(&mut rest, 0.1..=2.0).text("Rest Length (m)")).changed();
            let k_changed = ui.add(egui::Slider::new(&mut k, 100.0..=100000.0).text("Spring K (N/m)").logarithmic(true)).changed();
            let d_changed = ui.add(egui::Slider::new(&mut d, 100.0..=10000.0).text("Damping C (N·s/m)").logarithmic(true)).changed();
            let r_changed = ui.add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Wheel Radius (m)")).changed();

            if rest_changed || k_changed || d_changed || r_changed {
                if let Ok(mut wheel) = world.query::<&mut WheelRaycast>().get_mut(world, entity) {
                    if rest_changed { wheel.rest_length = rest as f64; }
                    if k_changed { wheel.spring_k = k as f64; }
                    if d_changed { wheel.damping_c = d as f64; }
                    if r_changed { wheel.wheel_radius = radius as f64; }
                }
            }
        }

        // Delete button
        ui.separator();
        if ui.button("🗑 Delete Entity (Del)").clicked() {
            if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                undo.push(UndoAction::Spawned { entity });
            }
            if world.get_entity(entity).is_ok() {
                world.commands().entity(entity).despawn();
            }
            if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                selected.entity = None;
            }
        }
    }
}
