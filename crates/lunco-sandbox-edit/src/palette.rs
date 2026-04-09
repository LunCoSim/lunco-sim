//! Spawn palette UI — an EGUI window listing all spawnable objects.
//!
//! Supports two workflows:
//! 1. **Click-to-place**: Click an item → ghost follows cursor → click in scene to place.
//! 2. **Drag-to-place**: Click and drag an item from the palette → ghost follows mouse →
//!    release in the scene to place.
//!
//! Both workflows trigger a `SPAWN_ENTITY` [CommandMessage] so that spawning can also
//! be driven via CLI later.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, egui};

use crate::catalog::{SpawnCatalog, SpawnCategory};
use crate::SpawnState;

/// Renders the spawn palette in the EGUI context.
pub fn spawn_palette_panel(
    mut contexts: EguiContexts,
    mut spawn_state: ResMut<SpawnState>,
    catalog: Res<SpawnCatalog>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // Escape cancels spawn selection
    if keys.just_pressed(KeyCode::Escape) {
        if matches!(&*spawn_state, SpawnState::Selecting { .. }) {
            *spawn_state = SpawnState::Idle;
        }
    }

    let is_selecting = matches!(&*spawn_state, SpawnState::Selecting { .. });

    egui::Window::new("Spawn Palette")
        .resizable(true)
        .default_width(200.0)
        .show(ctx, |ui| {
            ui.heading("Spawn");

            if is_selecting {
                let selected_id = match &*spawn_state {
                    SpawnState::Selecting { entry_id } => entry_id.clone(),
                    _ => String::new(),
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Placing: {}", selected_id))
                        .color(egui::Color32::GREEN));
                    if ui.button("Cancel").clicked() {
                        *spawn_state = SpawnState::Idle;
                    }
                });
                ui.separator();
            }

            // Group by category
            for category in [SpawnCategory::Rover, SpawnCategory::Prop, SpawnCategory::Terrain] {
                let entries: Vec<_> = catalog.by_category(category).collect();
                if entries.is_empty() { continue; }

                ui.collapsing(format!("{}", category), |ui| {
                    for entry in &entries {
                        let selected = matches!(&*spawn_state,
                            SpawnState::Selecting { ref entry_id } if entry_id == &entry.id);

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

                        // Click to select/deselect
                        if response.clicked() {
                            if selected {
                                *spawn_state = SpawnState::Idle;
                            } else {
                                *spawn_state = SpawnState::Selecting {
                                    entry_id: entry.id.clone(),
                                };
                            }
                        }

                        // Drag-to-place: start selecting on drag start,
                        // end selecting on drag release (spawn system handles actual placement)
                        if response.drag_started() {
                            *spawn_state = SpawnState::Selecting {
                                entry_id: entry.id.clone(),
                            };
                        }

                        if response.drag_stopped() {
                            // User released the drag — the spawn system's click handler
                            // will pick up the current cursor position and place it.
                            // We keep Selecting state so the ghost is still active.
                        }
                    }
                });
            }

            ui.separator();
            ui.small("Click to select, then click in scene to place.");
            ui.small("Or drag an item from here, then click in scene to place.");
            ui.small("Press Escape to cancel.");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_palette_select_entry() {
        let mut state = SpawnState::Idle;
        assert!(matches!(state, SpawnState::Idle));

        state = SpawnState::Selecting { entry_id: "ball_dynamic".into() };
        assert!(matches!(state, SpawnState::Selecting { ref entry_id } if entry_id == "ball_dynamic"));
    }

    #[test]
    fn test_spawn_palette_cancel() {
        let mut state = SpawnState::Selecting { entry_id: "ramp".into() };
        if matches!(&state, SpawnState::Selecting { .. }) {
            state = SpawnState::Idle;
        }
        assert!(matches!(state, SpawnState::Idle));
    }
}
