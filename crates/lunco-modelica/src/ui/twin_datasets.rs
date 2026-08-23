//! Twin-scoped downloadable resources for the lunica Twin Browser.
//!
//! The section is a pure projection of the shared [`DatasetRegistry`]. It
//! does not know URLs, cache paths, processing commands, or domain-specific
//! asset names. A row emits the same typed request used by Settings, so an
//! individual Twin resource follows the exact same downloader, watchdog, and
//! processing lifecycle.

use bevy_egui::egui;
use lunco_assets::datasets::{
    CancelDataset, DatasetEntry, DatasetRegistry, DatasetScope, DatasetState, RequestDataset,
};
use lunco_workbench::twin_browser::{BrowserCtx, BrowserScope, BrowserSection};

/// Twin Browser section that reports the active Twin's declared resources.
#[derive(Default)]
pub struct TwinDatasetsSection;

impl BrowserSection for TwinDatasetsSection {
    fn id(&self) -> &str {
        "lunco.assets.twin_datasets"
    }

    fn title(&self) -> &str {
        "Twin resources"
    }

    fn scope(&self) -> BrowserScope {
        BrowserScope::Models
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        150
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let Some(workspace) = ctx.resource::<lunco_workspace::WorkspaceResource>() else {
            ui.label(
                egui::RichText::new("Workspace unavailable.")
                    .weak()
                    .italics(),
            );
            return;
        };
        let Some(active_twin) = workspace.active_twin else {
            ui.label(
                egui::RichText::new("Open a Twin to inspect its downloadable resources.")
                    .weak()
                    .italics(),
            );
            return;
        };
        let Some(twin) = workspace.twin(active_twin) else {
            ui.label(egui::RichText::new("The active Twin is no longer available.").weak());
            return;
        };
        let twin_root = twin.root.clone();

        let Some(roots) = ctx.resource::<lunco_assets::TwinRoots>() else {
            ui.label(
                egui::RichText::new("Twin asset services are not installed in this host.")
                    .weak()
                    .italics(),
            );
            return;
        };
        let Some(twin_name) = roots.name_for_root(&twin_root) else {
            ui.label(
                egui::RichText::new("Mounting Twin asset resources…")
                    .weak()
                    .italics(),
            );
            return;
        };

        let Some(registry) = ctx.resource::<DatasetRegistry>() else {
            ui.label(
                egui::RichText::new("Dataset services are not installed in this host.")
                    .weak()
                    .italics(),
            );
            return;
        };

        let scope_matches = |scope: &DatasetScope| matches!(scope, DatasetScope::Twin { name, .. } if name == &twin_name);
        let scanned = registry.scanned_scopes().iter().any(scope_matches);
        if !scanned {
            ui.label(
                egui::RichText::new("Inspecting this Twin's Assets.toml…")
                    .weak()
                    .italics(),
            );
            return;
        }

        let mut rows: Vec<&DatasetEntry> = registry
            .entries()
            .iter()
            .filter(|entry| scope_matches(&entry.scope))
            .collect();
        rows.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.key.cmp(&right.key))
        });

        if rows.is_empty() {
            ui.label(
                egui::RichText::new("This Twin declares no downloadable resources.")
                    .weak()
                    .italics(),
            );
            return;
        }

        let installed = rows
            .iter()
            .filter(|entry| entry.state.is_installed())
            .count();
        ui.label(
            egui::RichText::new(format!("{installed}/{} ready", rows.len()))
                .weak()
                .small(),
        );

        enum Action {
            Request(String),
            Cancel(String),
        }
        let mut action = None;
        for entry in rows {
            ui.horizontal(|ui| {
                ui.label(&entry.name)
                    .on_hover_text(format!("{} · {}", entry.key, entry.group));
                match &entry.state {
                    DatasetState::Installed => {
                        ui.label(egui::RichText::new("Ready · cached").weak());
                    }
                    DatasetState::Missing => {
                        ui.label(egui::RichText::new("Not downloaded").weak());
                        if ui.button("Download").clicked() {
                            action = Some(Action::Request(entry.id.clone()));
                        }
                    }
                    DatasetState::Downloading {
                        bytes_done,
                        bytes_total,
                    } => {
                        let progress = (*bytes_total > 0)
                            .then_some((*bytes_done as f32 / *bytes_total as f32).clamp(0.0, 1.0));
                        if let Some(progress) = progress {
                            ui.add(egui::ProgressBar::new(progress).desired_width(110.0).text(
                                format!(
                                    "{:.1}/{:.1} MB",
                                    *bytes_done as f64 / 1_048_576.0,
                                    *bytes_total as f64 / 1_048_576.0
                                ),
                            ));
                        } else {
                            ui.label("Downloading…");
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some(Action::Cancel(entry.id.clone()));
                        }
                    }
                    DatasetState::Processing { kind } => {
                        ui.label(format!("Preparing locally ({kind})…"));
                    }
                    DatasetState::Failed(error) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("Failed: {error}"));
                        if ui.button("Retry").clicked() {
                            action = Some(Action::Request(entry.id.clone()));
                        }
                    }
                }
            });
        }

        if let Some(action) = action {
            match action {
                Action::Request(id) => ctx.trigger(RequestDataset { id }),
                Action::Cancel(id) => ctx.trigger(CancelDataset { id }),
            }
        }
    }
}
