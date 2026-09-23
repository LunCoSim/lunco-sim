//! Read-only sky-time display for scenes that declare celestial bodies.
//!
//! Celestial presentation follows the physical tick. This widget reports the
//! interpolated render sample; pause, rate, and mission epoch are owned by the
//! simulation time controls and authored scene epoch.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use lunco_celestial::CelestialBody;
use lunco_time::SimulationPresentationTime;
use lunco_workbench_core::MenuCtx;

fn sky_time_ui(ui: &mut egui::Ui, utc: &str, epoch_jd: f64) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Sky").weak().size(11.0));
        ui.label(egui::RichText::new(utc).monospace().size(11.0))
            .on_hover_text(format!("JD {epoch_jd:.6} (TDB), interpolated from physical ticks"));
    });
}

/// The Time menu's sky-time readout. Draws nothing when the scene has no
/// celestial bodies, matching the floating display.
pub(crate) fn sky_clock_menu_ui(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    if !ctx.has_component::<CelestialBody>() {
        ui.label(egui::RichText::new("No sky in this scene").weak().small());
        return;
    }
    let Some(time) = ctx.resource::<SimulationPresentationTime>().copied() else {
        return;
    };
    let utc = lunco_time::tdb_jd_to_utc_string(time.epoch_jd);
    sky_time_ui(ui, &utc, time.epoch_jd);
}

/// Paint the sky-time pill (top-left, under the view switcher). It only displays
/// the physical presentation sample and has no independent transport.
pub(crate) fn draw_celestial_time(
    mut egui_ctx: EguiContexts,
    q_bodies: Query<(), With<CelestialBody>>,
    time: Option<Res<SimulationPresentationTime>>,
) {
    if q_bodies.is_empty() {
        return;
    }
    let Some(time) = time else { return };
    let utc = lunco_time::tdb_jd_to_utc_string(time.epoch_jd);
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    egui::Area::new(egui::Id::new("celestial_time"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 40.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| sky_time_ui(ui, &utc, time.epoch_jd));
        });
}
