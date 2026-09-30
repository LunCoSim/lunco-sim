//! Spawn palette panel — `lunco-workbench::Panel` implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! The panel lists spawnable objects by category and supports click/drag to select.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};
use std::collections::HashMap;

use lunco_luncosim_edit_core::SpawnState;
use lunco_scene_catalog::catalog::{AssetMetaStore, SpawnCatalog, SpawnSource};

/// Replace the spawn-palette state after the panel's paint pass.
#[derive(Event)]
pub(crate) struct SpawnStateRequested(pub(crate) SpawnState);

pub(crate) fn on_spawn_state_requested(
    trigger: On<SpawnStateRequested>,
    mut state: ResMut<SpawnState>,
) {
    *state = trigger.0.clone();
}

/// Spawn palette panel — lists spawnable objects by category.
#[derive(Default)]
pub struct SpawnPalette {
    catalog_revision: Option<u64>,
    row_labels: HashMap<usize, String>,
}

impl Panel for SpawnPalette {
    fn id(&self) -> PanelId {
        PanelId("spawn_palette")
    }
    fn title(&self) -> String {
        "Spawn".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Builder
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some((success, success_subdued)) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| (theme.tokens.success, theme.tokens.success_subdued))
        else {
            return;
        };
        ctx.panel_content_frame().show(ui, |ui| {
            spawn_palette_content(self, ui, ctx, success, success_subdued);
        });
    }
}

fn spawn_palette_content(
    panel: &mut SpawnPalette,
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    success: egui::Color32,
    success_subdued: egui::Color32,
) {
    ui.heading("Spawn");

    // Keep the selection borrowed during paint; own an id only when an action
    // is dispatched.
    let selecting_id = ctx.resource::<SpawnState>().and_then(|state| match state {
        SpawnState::Selecting { entry_id } => Some(entry_id.as_str()),
        _ => None,
    });
    let is_selecting = selecting_id.is_some();
    let mut requested_states = Vec::new();

    if is_selecting {
        if let Some(id) = selecting_id {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Placing:").color(success));
                ui.label(id);
                if ui.button("Cancel").clicked() {
                    requested_states.push(SpawnStateRequested(SpawnState::Idle));
                }
            });
            ui.separator();
        }
    }

    // Keep authored categories dynamic while avoiding a cloned catalog
    // snapshot on every Builder frame. Entry rows are borrowed only when their
    // category is expanded.
    let metadata = ctx.resource::<AssetMetaStore>();
    if let Some(catalog) = ctx.resource::<SpawnCatalog>() {
        if panel.catalog_revision != Some(catalog.revision()) {
            panel.catalog_revision = Some(catalog.revision());
            panel.row_labels.clear();
        }
        for (category, entry_indices) in catalog.category_groups() {
            ui.collapsing(category, |ui| {
                for &entry_index in entry_indices {
                    let entry = catalog
                        .entry_at(entry_index)
                        .expect("spawn category index points to a catalog entry");
                    let selected = selecting_id.as_deref() == Some(entry.id.as_str());

                    let btn_text = panel.row_labels.entry(entry_index).or_insert_with(|| {
                        format!("{} · {}", entry.display_name, entry.origin.label())
                    });

                    let btn = egui::Button::new(btn_text.as_str());
                    let btn = if selected {
                        btn.fill(success_subdued)
                    } else {
                        btn
                    };

                    let response = ui.add(btn);
                    // The catalog and tooltip share the same async USD metadata
                    // store. Show a hint only when the authored default prim has
                    // a non-empty standard USD `doc` field; absent metadata stays
                    // quiet instead of inventing a description or placeholder.
                    let response = if let Some(description) = metadata
                        .and_then(|store| match &entry.source {
                            SpawnSource::UsdFile(path) => store.description(path),
                        })
                        .filter(|description| !description.trim().is_empty())
                    {
                        response.on_hover_text(description)
                    } else {
                        response
                    };

                    if response.clicked() {
                        let entry_id = entry.id.clone();
                        requested_states.push(SpawnStateRequested(if selected {
                            SpawnState::Idle
                        } else {
                            SpawnState::Selecting { entry_id }
                        }));
                    }

                    if response.drag_started() {
                        let entry_id = entry.id.clone();
                        requested_states
                            .push(SpawnStateRequested(SpawnState::Selecting { entry_id }));
                    }
                }
            });
        }
    }
    for requested_state in requested_states {
        ctx.trigger(requested_state);
    }

    ui.separator();
    ui.label("Click an item, then click in scene to place.");
    ui.label("Or drag an item from here, then click in scene to place.");
    ui.label("Use Cancel to back out (Escape / Backspace by default).");
}
