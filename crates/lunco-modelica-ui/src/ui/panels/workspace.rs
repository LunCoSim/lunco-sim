//! Compact Modelica workspace start panel.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelScrollPolicy, PanelSlot,
};

/// Stable id for the Modelica workspace start panel.
pub const MODELICA_WORKSPACE_PANEL_ID: PanelId = PanelId("modelica_workspace");

/// Start actions shown in the center before a Modelica view is open.
pub struct ModelicaWorkspacePanel;

impl Panel for ModelicaWorkspacePanel {
    fn id(&self) -> PanelId {
        MODELICA_WORKSPACE_PANEL_ID
    }

    fn title(&self) -> String {
        "Modelica Workspace".into()
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Lunica
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let has_model_view = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.active_document)
            .is_some_and(|document| {
                ctx.resource::<crate::ui::document_context::ModelicaDocuments>()
                    .is_some_and(|documents| documents.host(document).is_some())
            });

        let mut create_model = false;
        let mut open_file = false;
        let mut open_welcome = false;

        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(560.0);
                ui.heading(
                    egui::RichText::new(if has_model_view {
                        "Modelica workspace"
                    } else {
                        "No Modelica model open"
                    })
                    .text_style(lunco_theme::TypographyRole::Title.text_style()),
                );
                ui.add_space(theme.spacing.item_spacing);
                ui.label(
                    egui::RichText::new(if has_model_view {
                        "Open another source from Twin, or create a model. Model views open in center tabs."
                    } else {
                        "Choose a model in the Twin browser, open a Modelica file, or start a new model."
                    })
                    .color(theme.tokens.text_subdued),
                );
                ui.add_space(theme.spacing.item_spacing * 2.0);
                (create_model, open_file) = super::render_start_actions(
                    ui,
                    ("New Model", "Create a new untitled Modelica model"),
                    ("Open Modelica File…", "Choose a Modelica source file to open"),
                );
                ui.add_space(theme.spacing.item_spacing);
                if ui.link("Learning paths and examples").clicked() {
                    open_welcome = true;
                }
            });
        });

        if create_model {
            ctx.trigger(crate::ui::commands::CreateNewScratchModel::default());
        }
        if open_file {
            ctx.trigger(lunco_workbench_file_ops::ShowOpenFilePicker {});
        }
        if open_welcome {
            ctx.trigger(lunco_workbench_core::commands::FocusPanel {
                id: crate::ui::panels::welcome::WELCOME_PANEL_ID
                    .as_str()
                    .to_owned(),
            });
        }
    }
}
