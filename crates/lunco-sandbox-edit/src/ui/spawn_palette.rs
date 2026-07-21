//! Spawn palette panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! The panel lists spawnable objects by category and supports click/drag to select.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::catalog::SpawnCatalog;
use crate::SpawnState;

/// Spawn palette panel — lists spawnable objects by category.
pub struct SpawnPalette;

impl Panel for SpawnPalette {
    fn id(&self) -> PanelId { PanelId("spawn_palette") }
    fn title(&self) -> String { "Spawn".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some((mantle, tokens)) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| (theme.colors.mantle, theme.tokens.clone()))
        else {
            return;
        };
        // Uses the workbench's shared mantle colour so the
        // panel body matches the colour of its tab header.
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| { spawn_palette_content(self, ui, ctx, &tokens); });
    }
}

fn spawn_palette_content(
    _panel: &mut SpawnPalette,
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    tokens: &lunco_theme::DesignTokens,
) {
        ui.heading("Spawn");

        // Read current state
        let is_selecting = ctx.resource::<SpawnState>()
            .map(|s| matches!(*s, SpawnState::Selecting { .. }))
            .unwrap_or(false);
        let selecting_id = ctx.resource::<SpawnState>()
            .and_then(|s| match s {
                SpawnState::Selecting { entry_id } => Some(entry_id.clone()),
                _ => None,
            });

        if is_selecting {
            if let Some(id) = &selecting_id {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Placing: {id}"))
                        .color(tokens.success));
                    if ui.button("Cancel").clicked() {
                        ctx.defer(|world| {
                            if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                                *state = SpawnState::Idle;
                            }
                        });
                    }
                });
                ui.separator();
            }
        }

        // Read catalog — group by whatever dynamic category labels exist
        // (derived from content folders), so new content needs no UI change.
        let categories: Vec<(String, Vec<_>)> = {
            let Some(catalog) = ctx.resource::<SpawnCatalog>() else { return };
            catalog.categories()
                .into_iter()
                .map(|cat| {
                    let entries: Vec<_> = catalog.by_category(&cat).cloned().collect();
                    (cat, entries)
                })
                .filter(|(_, entries)| !entries.is_empty())
                .collect()
        };

        for (category, entries) in categories {
            ui.collapsing(category.to_string(), |ui| {
                for entry in &entries {
                    let selected = ctx.resource::<SpawnState>()
                        .map(|s| matches!(s, SpawnState::Selecting { entry_id } if *entry_id == entry.id))
                        .unwrap_or(false);

                    let btn_text = if selected {
                        format!("✓ {}", entry.display_name)
                    } else {
                        entry.display_name.clone()
                    };

                    let btn = egui::Button::new(&btn_text);
                    let btn = if selected {
                        btn.fill(tokens.success_subdued)
                    } else {
                        btn
                    };

                    let response = ui.add(btn);

                    if response.clicked() {
                        let entry_id = entry.id.clone();
                        ctx.defer(move |world| {
                            if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                                if selected {
                                    *state = SpawnState::Idle;
                                } else {
                                    *state = SpawnState::Selecting { entry_id };
                                }
                            }
                        });
                    }

                    if response.drag_started() {
                        let entry_id = entry.id.clone();
                        ctx.defer(move |world| {
                            if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                                *state = SpawnState::Selecting { entry_id };
                            }
                        });
                    }
                }
            });
        }

        ui.separator();
        ui.small("Click to select, then click in scene to place.");
        ui.small("Or drag an item from here, then click in scene to place.");
        ui.small("Press Escape to cancel.");

        // Escape key handling
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && is_selecting {
            ctx.defer(|world| {
                if let Some(mut state) = world.get_resource_mut::<SpawnState>() {
                    *state = SpawnState::Idle;
                }
            });
        }
    }
