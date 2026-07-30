//! Download-progress overlay for the in-flight scenario sync (G2). Mirrors
//! `terrain_progress`: a centered egui card shown while
//! [`ScenarioDownloadStatus`] is active, auto-dismissed once every asset is
//! cached. Networking-only — the resource + its updater live in `lunco-networking`.

#![cfg(feature = "networking")]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use lunco_networking::scenario_sync::ScenarioDownloadStatus;

/// Centered card: "Downloading <name> — X/Y assets · a.b / c.d MB" with a bar.
/// Hidden (no-op) while `status.active` is false, i.e. once all assets are cached.
pub(crate) fn draw_scenario_download(
    mut egui_ctx: EguiContexts,
    status: Res<ScenarioDownloadStatus>,
) {
    if !status.active {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    // Keep the frame loop warm so the spinner + bar animate even when the scene
    // is otherwise idle behind the download.
    ctx.request_repaint();

    let mb = |b: u64| (b as f64) / (1024.0 * 1024.0);
    let fraction = status.fraction().unwrap_or(0.0);
    let title = if status.name.is_empty() {
        "Downloading scenario".to_string()
    } else {
        format!("Downloading {}", status.name)
    };

    egui::Area::new(egui::Id::new("scenario_download_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(24, 20))
                .show(ui, |ui| {
                    ui.set_min_width(300.0);
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(22.0));
                            ui.add_space(8.0);
                            ui.heading(title);
                        });
                        ui.add_space(8.0);
                        ui.label(format!(
                            "{} / {} assets",
                            status.assets_done, status.assets_total
                        ));
                        ui.add_space(10.0);
                        ui.add(
                            egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                                .desired_width(260.0)
                                .text(format!(
                                    "{:.1} / {:.1} MB",
                                    mb(status.bytes_done),
                                    mb(status.bytes_total)
                                )),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Fetching scenario assets from server…")
                                .weak()
                                .size(11.0),
                        );
                    });
                });
        });
}
