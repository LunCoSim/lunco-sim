//! Built-in **Files** section — flat, domain-agnostic listing of every
//! file the [`lunco_twin::Twin`] indexer found.
//!
//! Always present in the Twin Browser. Defaults to *collapsed* because
//! the per-domain sections (Modelica, USD, …) are usually what the
//! user wants; Files is the escape hatch for "show me the raw layout."
//!
//! Click a row → emits [`super::BrowserAction::OpenFile`]. The host
//! app's domain dispatchers decide what "open" means per file kind
//! (Modelica → diagram tab, USD → stage tab, image → external viewer,
//! …). The Files section itself is intentionally dumb about file
//! semantics.

use bevy_egui::egui;

use super::{BrowserAction, BrowserCtx, BrowserSection};

/// The built-in Files section impl.
#[derive(Default)]
pub struct FilesSection;

impl BrowserSection for FilesSection {
    fn id(&self) -> &str {
        "lunco.workbench.files"
    }

    fn title(&self) -> &str {
        "Files"
    }

    fn default_open(&self) -> bool {
        // Domain sections (Modelica, USD) carry the primary navigation
        // story. Files is the escape hatch — let the user opt into it.
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx) {
        // Render workspace documents (saved + unsaved) so the list
        // stays stable across Save — a Save shouldn't make a doc
        // disappear from the user's view of "what am I working on."
        // Unsaved drafts get a `●` orange dirty-dot + italic name;
        // saved docs render as plain rows with the same kind badge.
        let docs: Vec<super::UnsavedDocEntry> = ctx
            .world
            .get_resource::<super::UnsavedDocs>()
            .map(|r| r.entries.clone())
            .unwrap_or_default();
        if !docs.is_empty() {
            ui.label(
                egui::RichText::new("Workspace")
                    .small()
                    .weak()
                    .strong(),
            );
            for entry in &docs {
                ui.horizontal(|ui| {
                    if entry.is_unsaved {
                        ui.label(
                            egui::RichText::new("●")
                                .color(egui::Color32::from_rgb(220, 160, 60)),
                        );
                        ui.label(
                            egui::RichText::new(&entry.display_name).italics(),
                        );
                    } else {
                        ui.label(egui::RichText::new("  "));
                        ui.label(egui::RichText::new(&entry.display_name));
                    }
                    ui.label(
                        egui::RichText::new(format!("({})", entry.kind))
                            .small()
                            .weak(),
                    );
                });
            }
            ui.separator();
        }

        let Some(twin) = ctx.twin else {
            if docs.is_empty() {
                ui.label(
                    egui::RichText::new("Open a Twin or folder to browse files.")
                        .weak()
                        .italics(),
                );
            }
            return;
        };

        let files = twin.files();
        if files.is_empty() {
            if docs.is_empty() {
                ui.label(
                    egui::RichText::new("(no files found in this Twin)")
                        .weak()
                        .italics(),
                );
            }
            return;
        }

        // Plain flat list keyed by relative path. A future iteration can
        // add directory grouping; flat is the right shape for slice 1
        // (matches `Twin::files()` and exercises every action path).
        egui::ScrollArea::vertical()
            .id_salt("twin_browser_files_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for entry in files {
                    let label = entry.relative_path.display().to_string();
                    let resp = ui.selectable_label(false, label);
                    if resp.clicked() {
                        ctx.actions.push(BrowserAction::OpenFile {
                            relative_path: entry.relative_path.clone(),
                        });
                    }
                }
            });
    }
}
