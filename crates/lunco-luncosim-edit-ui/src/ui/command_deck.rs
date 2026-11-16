//! Command Deck panel for generic selection and control authority.
//!
//! Programs are authored Twin content and run through the generic Rhai program
//! surface. This panel therefore reports only selection and possession; it does
//! not classify a selected object as a rover, waypoint, or autopilot.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_control_core::ControlLink;
use lunco_core::GlobalEntityId;
use lunco_embodiment_core::roles::TheLocalEmbodiment;
use lunco_scene_selection::SelectedEntities;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchSnapshot};

const COMMAND_DECK_PANEL_ID: PanelId = PanelId("command_deck");

#[derive(Resource, Default, Clone)]
pub struct CommandDeckView {
    pub selected: Option<Entity>,
    pub selected_label: String,
    pub driving: bool,
    pub control_label: String,
    pub can_request: bool,
}

impl lunco_core::SceneResource for CommandDeckView {}

pub(super) fn command_deck_visible(workbench: Option<Res<WorkbenchSnapshot>>) -> bool {
    workbench.is_some_and(|snapshot| snapshot.is_panel_visible(COMMAND_DECK_PANEL_ID))
}

pub fn populate_command_deck_view(
    mut view: ResMut<CommandDeckView>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalEmbodiment>,
    q_link: Query<&ControlLink>,
    q_name: Query<&Name>,
    q_callsign: Query<&lunco_core::markers::Callsign>,
    q_catalog_id: Query<&lunco_core::CatalogEntryId>,
    q_gid: Query<&GlobalEntityId>,
    q_inputs: Query<
        &lunco_port_core::InputPorts,
        Without<lunco_embodiment_core::roles::Embodiment>,
    >,
    q_parents: Query<&ChildOf>,
    q_preview: Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    sessions: Res<lunco_core_session::SessionRegistry>,
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
        (Some(target), Some(avatar)) => q_link.get(avatar).is_ok_and(|link| link.target == target),
        _ => false,
    };
    view.can_request = selection.is_some_and(|target| {
        q_inputs
            .get(target)
            .is_ok_and(|ports| !ports.values.is_empty())
            && !lunco_usd_bevy_scene::is_preview_only(target, &q_parents, &q_preview)
    });
    view.control_label = if view.driving {
        "Controlled by you".into()
    } else if !view.can_request {
        "No live writable control endpoint".into()
    } else if let Some(owner) = selection
        .and_then(|target| q_gid.get(target).ok())
        .and_then(|id| sessions.owner_of(id.get()))
    {
        format!(
            "Owned by session {} · request subject to authority",
            owner.0
        )
    } else {
        "Not controlled locally · request subject to authority".into()
    };
}

pub struct CommandDeck;

impl Panel for CommandDeck {
    fn id(&self) -> PanelId {
        COMMAND_DECK_PANEL_ID
    }

    fn title(&self) -> String {
        "Command Deck".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Builder
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

        ui.horizontal_wrapped(|ui| {
            ui.label("Selected:");
            ui.strong(if view.selected_label.is_empty() {
                "(unnamed)"
            } else {
                &view.selected_label
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Control:");
            ui.label(&view.control_label);
        });

        ui.separator();
        if view.driving {
            if ui.button("Release control").clicked() {
                if let Some(avatar) = ctx
                    .resource::<TheLocalEmbodiment>()
                    .and_then(|value| value.0)
                {
                    ctx.trigger(lunco_control_core::ReleaseControlSource { source: avatar });
                }
            }
        } else if ui.add_enabled(view.can_request, egui::Button::new("Request control"))
            .on_hover_text("The control authority checks ownership and permissions. A rejected request is reported in Recent events.")
            .clicked() {
            ctx.trigger(lunco_control_core::commands::AcquireControl {
                source: None,
                target,
                bind_camera: true,
            });
        }
    }
}
