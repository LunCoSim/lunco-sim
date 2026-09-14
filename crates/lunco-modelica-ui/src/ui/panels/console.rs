//! Modelica workbench console panel.
//!
//! The log model and renderer live in [`lunco_ui::log`]. This module only
//! supplies the Modelica panel identity and the clear event; other workbench
//! surfaces can reuse the same log resource and renderer without depending on
//! Modelica.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_ui::log::{render_log_view, LogBuffer};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

/// Panel id.
pub const CONSOLE_PANEL_ID: PanelId = PanelId("modelica_console");

/// Clear the console after the panel has completed its read-only paint pass.
#[derive(Event, Clone, Copy, Default)]
pub(crate) struct ClearConsoleRequested;

pub(crate) fn on_clear_console_requested(
    _trigger: On<ClearConsoleRequested>,
    mut log: ResMut<LogBuffer>,
) {
    log.clear();
}

/// Modelica workbench console panel.
pub struct ConsolePanel;

impl Panel for ConsolePanel {
    fn id(&self) -> PanelId {
        CONSOLE_PANEL_ID
    }

    fn title(&self) -> String {
        "Console".into()
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Design
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let muted = theme.tokens.text_subdued;
        let snapshot = ctx
            .resource::<LogBuffer>()
            .map(|log| log.entries().clone())
            .unwrap_or_default();
        let mut clear_requested = false;
        let _ = render_log_view(
            ui,
            &snapshot,
            "(no messages yet — compile a model, save, or open a folder)",
            &mut clear_requested,
            muted,
            &theme,
        );
        if clear_requested {
            ctx.trigger(ClearConsoleRequested);
        }
    }
}
