//! Command Deck panel for generic selection and control authority.
//!
//! Programs are authored Twin content and run through the generic Rhai program
//! surface. This panel therefore reports only selection and possession; it does
//! not classify a selected object as a rover, waypoint, or autopilot.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_controller::ControllerLink;
use lunco_core::{GlobalEntityId, TheLocalAvatar};
use lunco_scene_commands::SelectedEntities;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

#[derive(Resource, Default, Clone)]
pub struct CommandDeckView {
    pub selected: Option<Entity>,
    pub selected_label: String,
    pub driving: bool,
}

pub fn populate_command_deck_view(
    mut view: ResMut<CommandDeckView>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalAvatar>,
    q_link: Query<&ControllerLink>,
    q_name: Query<&Name>,
    q_callsign: Query<&lunco_core::markers::Callsign>,
    q_catalog_id: Query<&lunco_core::CatalogEntryId>,
    q_gid: Query<&GlobalEntityId>,
) {
    let selection = selected.primary();
    view.selected = selection;
    view.selected_label = selection
        .map(|entity| {
            lunco_core::entity_display_name(
                q_name.get(entity).ok(),
                q_callsign.get(entity).ok(),
                q_catalog_id.get(entity).ok(),
            )
        })
        .filter(|label| !label.is_empty())
        .or_else(|| {
            selection
                .and_then(|entity| q_gid.get(entity).ok())
                .map(|id| format!("entity #{}", id.get()))
        })
        .unwrap_or_default();
    view.driving = match (selection, local_avatar.0) {
        (Some(target), Some(avatar)) => q_link
            .get(avatar)
            .is_ok_and(|link| link.vessel_entity == target),
        _ => false,
    };
}

pub struct CommandDeck;

impl Panel for CommandDeck {
    fn id(&self) -> PanelId {
        PanelId("command_deck")
    }

    fn title(&self) -> String {
        "Command Deck".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Tools
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ui.heading("Command Deck");
        ui.separator();

        let Some(view) = ctx.resource::<CommandDeckView>().cloned() else {
            ui.weak("No command state");
            return;
        };
        let Some(target) = view.selected else {
            ui.weak("Select an entity in the scene");
            return;
        };

        ui.horizontal(|ui| {
            ui.label("Selected:");
            ui.strong(if view.selected_label.is_empty() {
                "(unnamed)"
            } else {
                &view.selected_label
            });
        });
        ui.horizontal(|ui| {
            ui.label("Control:");
            if view.driving {
                ui.label("Local avatar");
            } else {
                ui.weak("Available");
            }
        });

        ui.separator();
        if view.driving {
            if ui.button("Release control").clicked() {
                if let Some(avatar) = ctx.resource::<TheLocalAvatar>().and_then(|value| value.0) {
                    ctx.trigger(lunco_avatar::ReleaseVessel { target: avatar });
                }
            }
        } else if ui.button("Take control").clicked() {
            ctx.trigger(lunco_avatar::PossessVessel {
                avatar: None,
                target,
                bind_camera: true,
            });
        }
        ui.small("Programs and route policy are authored by the active Twin.");
    }
}
