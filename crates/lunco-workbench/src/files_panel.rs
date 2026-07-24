//! Files Panel — raw on-disk view of the active Twin / open Folder.
//!
//! Sibling of [`TwinBrowserPanel`](crate::TwinBrowserPanel) (which
//! shows typed Twin content like Modelica classes and drafts) and
//! [`LibraryPanel`](crate::LibraryPanel) (which shows app-level
//! reference content like MSL). The three panels tab together in the
//! side dock by default — separate tabs rather than sub-tabs of one
//! browser, matching peer tools that keep "what's in this project",
//! "what's on disk", and "what's available globally" as distinct
//! navigation surfaces (Unity Project window vs Packages, Unreal
//! Content vs Engine Content, Simulink Current Folder vs Library
//! Browser).
//!
//! Renders every section with
//! [`scope`](crate::twin_browser::BrowserSection::scope) equal to
//! [`BrowserScope::Files`]. The workbench's built-in
//! [`FilesSection`](crate::FilesSection) is the default content;
//! domain crates may add more (e.g. a future
//! `WorkspaceDocumentsSection` surfacing a categorised list of
//! every open document, saved + unsaved).

use bevy::prelude::*;
use bevy_egui::egui;

use crate::panel::{Panel, PanelCtx, PanelId, PanelSlot};
use crate::twin_browser::{BrowserActions, BrowserCtx, BrowserScope, BrowserSectionRegistry};

/// Stable id of the Files panel.
pub const FILES_PANEL_ID: PanelId = PanelId("lunco.workbench.files");

/// The Files panel singleton.
#[derive(Default)]
pub struct FilesPanel;

impl Panel for FilesPanel {
    fn id(&self) -> PanelId {
        FILES_PANEL_ID
    }

    fn title(&self) -> String {
        "Files".to_string()
    }

    fn menu_group(&self) -> crate::PanelMenuGroup {
        crate::PanelMenuGroup::Design
    }

    fn default_slot(&self) -> PanelSlot {
        // Hidden by default: the Twin Browser panel now renders the
        // Files section inline as a sibling of the Modelica section,
        // CATIA-style. FilesPanel remains registered (Floating slot =
        // registered-but-not-docked) so users / a future View menu
        // can still surface it as its own dock tab if they prefer the
        // separate-panel layout.
        PanelSlot::Floating
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Scope the section registry + action outbox out of the world for the
        // duration of the render — the narrow, structural way to get `&mut` to
        // the registry's trait objects (no raw `&mut World`). Mirrors
        // `TwinBrowserPanel::render`; sections read domain state (active Twin
        // via `WorkspaceResource`, …) through `BrowserCtx::resource` and emit
        // changes via `defer`/`actions`.
        let present = ctx.resource_scope::<BrowserSectionRegistry, _>(|ctx, registry| {
            let visible: Vec<usize> = registry
                .iter()
                .enumerate()
                .filter(|(_, s)| s.scope() == BrowserScope::Files)
                .map(|(i, _)| i)
                .collect();

            if visible.is_empty() {
                ui.label(
                    egui::RichText::new("No file sections registered.")
                        .weak()
                        .italics(),
                );
                return;
            }

            ctx.resource_scope::<BrowserActions, _>(|ctx, actions| {
                // Wrap every section in ONE panel-level ScrollArea — same as the
                // inline Twin browser (`twin_browser/mod.rs`). Without it a long
                // file list (or a fully-expanded folder tree) overflows the panel
                // rect with no way to scroll = "tons of files but can't see them".
                // Sections render their lists DIRECTLY (no nested vertical
                // ScrollArea) so this outer one owns all scrolling; nesting two
                // vertical scroll areas squishes the inner to a few rows.
                egui::ScrollArea::vertical()
                    .id_salt("files_panel_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for &i in &visible {
                            let section = registry.section_mut(i);
                            let header = egui::CollapsingHeader::new(section.title())
                                .id_salt(("files_panel_section", section.id()))
                                .default_open(section.default_open());
                            header.show(ui, |ui| {
                                let mut bctx = BrowserCtx::new(&mut *actions, &mut *ctx);
                                section.render(ui, &mut bctx);
                            });
                        }
                    });
            });
        });

        if present.is_none() {
            let error_color = ctx
                .resource::<lunco_theme::Theme>()
                .map(|t| t.tokens.error)
                .unwrap_or(egui::Color32::LIGHT_RED);
            ui.colored_label(error_color, "BrowserSectionRegistry resource missing");
        }
    }
}
