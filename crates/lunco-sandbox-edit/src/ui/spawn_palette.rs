//! Spawn palette panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! The panel lists spawnable objects by category and supports click/drag to select.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::catalog::{SpawnCatalog, SpawnCategory};
use crate::SpawnState;

/// Spawn palette panel — lists spawnable objects by category.
pub struct SpawnPalette;

impl WorkbenchPanel for SpawnPalette {
    fn id(&self) -> &str { "spawn_palette" }
    fn title(&self) -> String { "Spawn".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Spawn palette requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Draw opaque background for this panel
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.heading("Spawn");

        // Read current state
        let is_selecting = world.get_resource::<SpawnState>()
            .map(|s| matches!(*s, SpawnState::Selecting { .. }))
            .unwrap_or(false);
        let selecting_id = world.get_resource::<SpawnState>()
            .and_then(|s| match &*s {
                SpawnState::Selecting { entry_id } => Some(entry_id.clone()),
                _ => None,
            });

        if is_selecting {
            if let Some(id) = &selecting_id {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Placing: {id}"))
                        .color(egui::Color32::GREEN));
                    if ui.button("Cancel").clicked() {
                        if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                            *state = SpawnState::Idle;
                        }
                    }
                });
                ui.separator();
            }
        }

        // Read catalog
        let Some(catalog) = world.get_resource::<SpawnCatalog>() else { return };
        let categories: Vec<_> = [
            SpawnCategory::Rover, SpawnCategory::Component,
            SpawnCategory::Prop, SpawnCategory::Terrain,
        ].into_iter()
            .filter_map(|cat| {
                let entries: Vec<_> = catalog.by_category(cat).cloned().collect();
                if entries.is_empty() { None } else { Some((cat, entries)) }
            })
            .collect();

        for (category, entries) in categories {
            ui.collapsing(format!("{category}"), |ui| {
                for entry in &entries {
                    let selected = world.get_resource::<SpawnState>()
                        .map(|s| matches!(&*s, SpawnState::Selecting { ref entry_id } if entry_id == &entry.id))
                        .unwrap_or(false);

                    let btn_text = if selected {
                        format!("✓ {}", entry.display_name)
                    } else {
                        entry.display_name.clone()
                    };

                    let btn = egui::Button::new(&btn_text);
                    let btn = if selected {
                        btn.fill(egui::Color32::DARK_GREEN)
                    } else {
                        btn
                    };

                    let response = ui.add(btn);

                    if response.clicked() {
                        if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                            if selected {
                                *state = SpawnState::Idle;
                            } else {
                                *state = SpawnState::Selecting {
                                    entry_id: entry.id.clone(),
                                };
                            }
                        }
                    }

                    if response.drag_started() {
                        if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                            *state = SpawnState::Selecting {
                                entry_id: entry.id.clone(),
                            };
                        }
                    }
                }
            });
        }

        ui.separator();
        ui.small("Click to select, then click in scene to place.");
        ui.small("Or drag an item from here, then click in scene to place.");
        ui.small("Press Escape to cancel.");

        // Escape key handling
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if is_selecting {
                if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                    *state = SpawnState::Idle;
                }
            }
        }
    }
}
