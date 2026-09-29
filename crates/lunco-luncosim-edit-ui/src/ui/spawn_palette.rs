//! Spawn palette panel — `lunco-workbench::Panel` implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! The panel lists spawnable objects by category and supports click/drag to select.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

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
pub struct SpawnPalette;

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
        let Some(tokens) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| theme.tokens.clone())
        else {
            return;
        };
        ctx.panel_content_frame().show(ui, |ui| {
            spawn_palette_content(self, ui, ctx, &tokens);
        });
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
    let is_selecting = ctx
        .resource::<SpawnState>()
        .map(|s| matches!(*s, SpawnState::Selecting { .. }))
        .unwrap_or(false);
    let selecting_id = ctx.resource::<SpawnState>().and_then(|s| match s {
        SpawnState::Selecting { entry_id } => Some(entry_id.clone()),
        _ => None,
    });

    if is_selecting {
        if let Some(id) = &selecting_id {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Placing: {id}")).color(tokens.success));
                if ui.button("Cancel").clicked() {
                    ctx.trigger(SpawnStateRequested(SpawnState::Idle));
                }
            });
            ui.separator();
        }
    }

    // Keep authored categories dynamic while avoiding a cloned catalog
    // snapshot on every Builder frame. Entry rows are borrowed only when their
    // category is expanded.
    let mut requested_states = Vec::new();
    if let Some(catalog) = ctx.resource::<SpawnCatalog>() {
        for category in catalog.categories() {
            ui.collapsing(category.clone(), |ui| {
                for entry in catalog.by_category(&category) {
                    let selected = selecting_id.as_deref() == Some(entry.id.as_str());

                    let btn_text = format!("{} · {}", entry.display_name, entry.origin.label());

                    let btn = egui::Button::new(&btn_text);
                    let btn = if selected {
                        btn.fill(tokens.success_subdued)
                    } else {
                        btn
                    };

                    let response = ui.add(btn);
                    // The catalog and tooltip share the same async USD metadata
                    // store. Show a hint only when the authored default prim has
                    // a non-empty standard USD `doc` field; absent metadata stays
                    // quiet instead of inventing a description or placeholder.
                    let response = if let Some(description) = ctx
                        .resource::<AssetMetaStore>()
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
    ui.small("Click an item, then click in scene to place.");
    ui.small("Or drag an item from here, then click in scene to place.");
    ui.small("Use Cancel to back out (Escape / Backspace by default).");
}
