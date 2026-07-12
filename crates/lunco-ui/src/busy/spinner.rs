//! Three-dot animated spinner used by [`super::widget::LoadingIndicator`].
//!
//! Extracted from the pre-existing canvas-diagram overlays so every
//! variant of the indicator (per-tab overlay card, inline tree row,
//! status-bar tick) shares one animation.

use bevy_egui::egui::{self, Color32, Pos2};

/// Paint three pulsing dots centred at `center`, animating with the
/// frame-time clock from `ui.ctx().input(|i| i.time)`.
///
/// `colour` is the fully-opaque base; the dots fade between 35% and
/// 100% alpha out of phase. The widget is purely cosmetic — it does
/// not allocate any interactable space.
pub(crate) fn paint_three_dot(ui: &mut egui::Ui, center: Pos2, colour: Color32) {
    let t = ui.ctx().input(|i| i.time) as f32;
    let painter = ui.painter();
    let radius = 3.5;
    let spacing = 12.0;
    for i in 0..3 {
        let phase = (t * 2.5 + i as f32 * 0.45).sin() * 0.5 + 0.5;
        let alpha = (0.35 + 0.65 * phase).clamp(0.0, 1.0);
        let dot_colour = Color32::from_rgba_unmultiplied(
            colour.r(),
            colour.g(),
            colour.b(),
            (alpha * 255.0) as u8,
        );
        let cx = center.x + (i as f32 - 1.0) * spacing;
        painter.circle_filled(Pos2::new(cx, center.y), radius, dot_colour);
    }
    // Keep animating while we're on screen.
    ui.ctx().request_repaint();
}
