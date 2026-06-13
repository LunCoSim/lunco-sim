//! Entity list panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Lists all named entities alphabetically; clicking one selects it.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::SelectedEntity;

/// Entity list panel — selectable list of scene entities.
pub struct EntityList;

impl Panel for EntityList {
    fn id(&self) -> PanelId { PanelId("entity_list") }
    fn title(&self) -> String { "Entities".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let (mantle, tokens) = {
            let theme = world.resource::<lunco_theme::Theme>();
            (theme.colors.mantle, theme.tokens.clone())
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| entity_list_content(self, ui, world, &tokens));
    }
}

fn entity_list_content(_panel: &mut EntityList, ui: &mut egui::Ui, world: &mut World, tokens: &lunco_theme::DesignTokens) {

        ui.label("Click to select an entity. Use gizmos to move it.");
        ui.separator();

        // Collect entity list (read-only, scoped borrow)
        let entities: Vec<(Entity, String)> = world.query::<(Entity, &Name)>().iter(world)
            .map(|(e, name)| (e, name.as_str().to_string()))
            .collect();

        // The editable-shader subset (terrain + props carrying a custom
        // `ShaderMaterial`). Pinned at the top so the terrain shader is one
        // click from the Explorer instead of buried in the full list.
        let shader_ents: Vec<(Entity, String)> = world
            .query_filtered::<(Entity, &Name), With<MeshMaterial3d<lunco_materials::ShaderMaterial>>>()
            .iter(world)
            .map(|(e, name)| (e, name.as_str().to_string()))
            .collect();

        // Sort by name
        let mut sorted: Vec<_> = entities.iter().collect();
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        let mut shader_sorted: Vec<_> = shader_ents.iter().collect();
        shader_sorted.sort_by(|a, b| a.1.cmp(&b.1));

        // Get current selection (separate borrow)
        let currently_selected = world
            .get_resource::<SelectedEntity>()
            .and_then(|s| s.entity);

        // Collect a click across either section; apply once at the end to keep
        // the world borrow simple.
        let mut to_select: Option<Entity> = None;

        // Renders one selectable row, recording a click into `to_select`.
        let mut row = |ui: &mut egui::Ui, entity: Entity, name: &str, to_select: &mut Option<Entity>| {
            let is_selected = currently_selected == Some(entity);
            let button = egui::Button::new(name);
            let button = if is_selected { button.fill(tokens.success_subdued) } else { button };
            if ui.add(button).clicked() {
                *to_select = Some(entity);
            }
        };

        if !shader_sorted.is_empty() {
            egui::CollapsingHeader::new("🎨 Shader materials")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Edit params in the Inspector").weak());
                    for (entity, name) in &shader_sorted {
                        row(ui, *entity, name, &mut to_select);
                    }
                });
            ui.separator();
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (entity, name) in &sorted {
                row(ui, *entity, name, &mut to_select);
            }
        });

        if let Some(entity) = to_select {
            if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                selected.entity = Some(entity);
            }
        }
    }
