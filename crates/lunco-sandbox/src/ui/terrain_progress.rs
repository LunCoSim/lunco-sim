//! Centered "Generating terrain…" overlay shown while the DEM bake is in flight.
//!
//! Loading a moonbase Twin first decodes a multi-thousand-sample GeoTIFF and
//! stamps ~100k analytic craters before the first mesh/collider appears — a few
//! seconds during which the viewport is an unexplained black rectangle. This
//! paints a centered card driven by [`TerrainGenStatus`] (derived from the
//! terrain build components in `lunco-terrain-surface`) so the wait reads as
//! progress, not a hang. It clears the instant the last tile finishes.
//!
//! Distinct from the bottom-bar `report_terrain_stream_status`, which reports the
//! *post-bake* CDLOD tile streaming — this covers the initial bake that blocks
//! anything appearing at all.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_terrain_surface::TerrainGenStatus;

/// Paint the centered generation card when a terrain build is active. Runs in
/// `EguiPrimaryContextPass`; a no-op (early return) whenever nothing is baking.
pub(crate) fn draw_terrain_progress(mut egui_ctx: EguiContexts, mut status: ResMut<TerrainGenStatus>) {
    if !status.active {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    // Keep the frame loop warm so the spinner + indeterminate bar animate even
    // when the scene is otherwise idle behind the bake.
    ctx.request_repaint();

    let phase = if status.site.is_empty() {
        status.phase.label().to_string()
    } else {
        format!("{} — {}", status.phase.label(), status.site)
    };

    egui::Area::new(egui::Id::new("terrain_gen_overlay"))
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
                            ui.heading("Generating terrain");
                        });
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(phase).size(14.0));
                        ui.add_space(10.0);
                        match status.fraction {
                            Some(f) => {
                                ui.add(
                                    egui::ProgressBar::new(f.clamp(0.0, 1.0))
                                        .desired_width(260.0)
                                        .show_percentage(),
                                );
                            }
                            None => {
                                // No incremental signal from the bake → indeterminate.
                                ui.add(
                                    egui::ProgressBar::new(0.0).desired_width(260.0).animate(true),
                                );
                            }
                        }
                        ui.add_space(4.0);
                        // Typed phase → caption (no fragile substring matching; the
                        // native "Baking" phase now shows the decode/stamp caption
                        // instead of falling through to "Preparing…").
                        let subtext = status.phase.caption();
                        ui.label(
                            egui::RichText::new(subtext)
                                .weak()
                                .size(11.0),
                        );
                        ui.add_space(12.0);
                        if ui.button("Dismiss Overlay").clicked() {
                            status.user_dismissed = true;
                            status.active = false;
                        }
                    });
                });
        });
}
