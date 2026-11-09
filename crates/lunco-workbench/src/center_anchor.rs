//! Persistent empty-state tab for center-driven workbench layouts.

use bevy_egui::egui;
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot, WORKBENCH_CENTER_ANCHOR_ID,
};

/// Workbench-owned center tab used when a perspective has no persistent
/// center panel of its own.
pub(crate) struct CenterAnchorPanel;

impl Panel for CenterAnchorPanel {
    fn id(&self) -> PanelId {
        WORKBENCH_CENTER_ANCHOR_ID
    }

    fn title(&self) -> String {
        "Workspace".into()
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Hidden
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Hidden
    }

    fn closable(&self) -> bool {
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        ui.vertical_centered(|ui| {
            ui.set_max_width(520.0);
            ui.add_space(theme.spacing.item_spacing * 6.0);
            ui.heading("Nothing open");
            ui.add_space(theme.spacing.item_spacing);
            ui.label(
                egui::RichText::new("Open a document, or choose a view from the View menu.")
                    .color(theme.tokens.text_subdued),
            );
        });
    }
}
