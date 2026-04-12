//! Overlay panel rendering for 3D-embedded mode.
//!
//! Panels render as egui::Window overlays with semi-transparent backgrounds,
//! floating on top of the 3D scene. No docking system — each panel is independent.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::{SpawnState, SelectedEntity, UndoStack, catalog::{SpawnCatalog, SpawnCategory}};

/// Overlay state tracking which panels are open.
/// Panels are open by default.
#[derive(Resource)]
pub struct OverlayState {
    pub spawn_open: bool,
    pub inspector_open: bool,
    pub entities_open: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            spawn_open: true,
            inspector_open: true,
            entities_open: true,
        }
    }
}

/// Renders all overlay panels on top of the 3D scene.
pub fn render_overlay_panels(
    mut contexts: EguiContexts,
    mut spawn_state: ResMut<SpawnState>,
    mut selected: ResMut<SelectedEntity>,
    mut undo_stack: ResMut<UndoStack>,
    mut overlay: ResMut<OverlayState>,
    catalog: Res<SpawnCatalog>,
    q_names: Query<(Entity, &Name)>,
    mut q_transforms: Query<&mut Transform>,
    q_rb: Query<&avian3d::prelude::RigidBody>,
    mut q_mass: Query<&mut avian3d::prelude::Mass>,
    mut q_lin_damp: Query<&mut avian3d::prelude::LinearDamping>,
    mut q_ang_damp: Query<&mut avian3d::prelude::AngularDamping>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Tab toggles all panels
    if keys.just_pressed(KeyCode::Tab) {
        let any_open = overlay.spawn_open || overlay.inspector_open || overlay.entities_open;
        overlay.spawn_open = !any_open;
        overlay.inspector_open = !any_open;
        overlay.entities_open = !any_open;
    }

    let bg = egui::Color32::from_rgba_unmultiplied(25, 25, 30, 230);
    let text = egui::Color32::from_rgb(230, 230, 240);

    let panel_size = [280.0, 500.0];

    // ── Spawn Palette ──
    egui::Window::new("Spawn")
        .open(&mut overlay.spawn_open)
        .default_size(panel_size)
        .resizable(true)
        .show(ctx, |ui| {
            apply_theme(ui, bg, text);

            let is_selecting = matches!(&*spawn_state, SpawnState::Selecting { .. });
            ui.heading("Spawn");

            if is_selecting {
                let entry_id = if let SpawnState::Selecting { entry_id } = &*spawn_state {
                    Some(entry_id.clone())
                } else { None };
                if let Some(ref id) = entry_id {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Placing: {id}"))
                            .color(egui::Color32::GREEN));
                        if ui.button("Cancel").clicked() {
                            *spawn_state = SpawnState::Idle;
                        }
                    });
                    ui.separator();
                }
            }

            for category in [SpawnCategory::Rover, SpawnCategory::Component, SpawnCategory::Prop, SpawnCategory::Terrain] {
                let entries: Vec<_> = catalog.by_category(category).collect();
                if entries.is_empty() { continue; }

                ui.collapsing(format!("{category}"), |ui| {
                    for entry in &entries {
                        let is_sel = matches!(&*spawn_state,
                            SpawnState::Selecting { ref entry_id } if entry_id == &entry.id);

                        let btn_text = if is_sel {
                            format!("✓ {}", entry.display_name)
                        } else {
                            entry.display_name.clone()
                        };

                        let btn = egui::Button::new(&btn_text);
                        let btn = if is_sel {
                            btn.fill(egui::Color32::DARK_GREEN)
                        } else {
                            btn
                        };

                        let response = ui.add(btn);

                        if response.clicked() {
                            if is_sel {
                                *spawn_state = SpawnState::Idle;
                            } else {
                                *spawn_state = SpawnState::Selecting {
                                    entry_id: entry.id.clone(),
                                };
                            }
                        }

                        if response.drag_started() {
                            *spawn_state = SpawnState::Selecting {
                                entry_id: entry.id.clone(),
                            };
                        }
                    }
                });
            }

            ui.separator();
            ui.small("Click to select, then click in scene to place.");
            ui.small("Or drag an item from here, then click in scene to place.");
            ui.small("Press Escape to cancel.");
        });

    // ── Inspector ──
    egui::Window::new("Inspector")
        .open(&mut overlay.inspector_open)
        .default_size(panel_size)
        .resizable(true)
        .show(ctx, |ui| {
            apply_theme(ui, bg, text);
            ui.heading("Inspector");

            let Some(entity) = selected.entity else {
                ui.label("No entity selected.");
                ui.label("Press Shift+Left-click on an object to select it.");
                return;
            };

            ui.label(format!("ID: {entity:?}"));
            if let Ok((_, name)) = q_names.get(entity) {
                ui.label(format!("Name: {}", name.as_str()));
            }

            ui.separator();
            ui.heading("Transform");

            // Copy transform out, edit, copy back (avoids borrow conflicts)
            if let Ok(tf) = q_transforms.get(entity) {
                let mut x = tf.translation.x;
                let mut y = tf.translation.y;
                let mut z = tf.translation.z;
                let changed = ui.add(egui::Slider::new(&mut x, -1000.0..=1000.0).text("X")).changed()
                    | ui.add(egui::Slider::new(&mut y, -1000.0..=1000.0).text("Y")).changed()
                    | ui.add(egui::Slider::new(&mut z, -1000.0..=1000.0).text("Z")).changed();
                if changed {
                    if let Ok(mut tf2) = q_transforms.get_mut(entity) {
                        tf2.translation = Vec3::new(x, y, z);
                    }
                }
            }

            ui.separator();
            ui.heading("Physics");

            if let Ok(rb) = q_rb.get(entity) {
                ui.label(format!("Type: {rb:?}"));
            }

            // Mass
            let mass_val = q_mass.get(entity).ok().map(|m| m.0);
            if let Some(mut m) = mass_val {
                if ui.add(egui::Slider::new(&mut m, 0.1..=100000.0).text("Mass (kg)").logarithmic(true)).changed() {
                    if let Ok(mut mass2) = q_mass.get_mut(entity) {
                        mass2.0 = m;
                    }
                }
            }

            // Linear damping
            let ld_val = q_lin_damp.get(entity).ok().map(|d| d.0 as f32);
            if let Some(mut d) = ld_val {
                if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping")).changed() {
                    if let Ok(mut d2) = q_lin_damp.get_mut(entity) {
                        d2.0 = d as f64;
                    }
                }
            }

            // Angular damping
            let ad_val = q_ang_damp.get(entity).ok().map(|d| d.0 as f32);
            if let Some(mut d) = ad_val {
                if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping")).changed() {
                    if let Ok(mut d2) = q_ang_damp.get_mut(entity) {
                        d2.0 = d as f64;
                    }
                }
            }

            ui.separator();
            if ui.button("🗑 Delete Entity (Del)").clicked() {
                undo_stack.push(crate::UndoAction::Spawned { entity });
                selected.entity = None;
            }
        });

    // ── Entity List ──
    egui::Window::new("Entities")
        .open(&mut overlay.entities_open)
        .default_size(panel_size)
        .resizable(true)
        .show(ctx, |ui| {
            apply_theme(ui, bg, text);
            ui.label("Click to select an entity.");
            ui.separator();

            let mut entities: Vec<_> = q_names.iter()
                .map(|(e, name)| (e, name.as_str().to_string()))
                .collect();
            entities.sort_by(|a, b| a.1.cmp(&b.1));

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (entity, name) in &entities {
                    let is_selected = selected.entity == Some(*entity);
                    let button = egui::Button::new(name);
                    let button = if is_selected {
                        button.fill(egui::Color32::DARK_GREEN)
                    } else {
                        button
                    };

                    if ui.add(button).clicked() {
                        selected.entity = Some(*entity);
                    }
                }
            });
        });
}

fn apply_theme(ui: &mut egui::Ui, bg: egui::Color32, text: egui::Color32) {
    let mut style = ui.style_mut().clone();
    style.visuals.panel_fill = bg;
    style.visuals.window_fill = bg;
    style.visuals.extreme_bg_color = bg;
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(40, 40, 50, 200);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(40, 40, 50, 200);
    style.visuals.override_text_color = Some(text);
}
